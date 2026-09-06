//! Regression tests for `expose_tools` enforcement symmetry.
//!
//! An operator's `expose_tools` allowlist must hide the same tools regardless of
//! how the upstream is authenticated. The catalog-backed (non-OAuth) path
//! filters through `UpstreamEntry::exposure_policy`; the OAuth subject-scoped
//! path has no catalog entry to consult and resolves the same policy from the
//! live `UpstreamConfig`. These tests pin both halves together so the two can
//! never drift again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::SubjectScopedConnection;
use super::entries::{healthy_in_process_entry, resolve_exposure_policy};
use super::testsupport::*;

/// The tools every fixture upstream advertises, and the allowlist applied to it.
const DISCOVERED_TOOLS: [&str; 3] = ["search_repos", "github_create_issue", "delete_repo"];
const EXPOSE_TOOLS: [&str; 2] = ["search_repos", "github_*"];
const EXPECTED_EXPOSED: [&str; 2] = ["github_create_issue", "search_repos"];

#[tokio::test]
async fn matching_subject_catalog_reports_authoritative_inspection_exhaustion() {
    let pool = static_catalog_pool("alpha").await;
    let mut tools = Vec::with_capacity(10_001);
    tools.push(test_tool("needle"));
    tools.extend((0..10_000).map(|index| test_tool(&format!("ordinary_{index:05}"))));
    move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", tools).await;
    let config = UpstreamConfig {
        expose_tools: None,
        ..oauth_upstream_config("alpha", &["*"])
    };
    let result = pool
        .subject_scoped_upstream_tools_allowed_matching_bounded(
            &[config],
            "alice",
            None,
            10_000,
            &|_, tool| tool.name.as_ref() == "needle",
            Duration::from_secs(1),
        )
        .await;
    assert_eq!(result.inspected, 10_000);
    assert!(result.incomplete);
    assert_eq!(result.tools.len(), 1);
    assert_eq!(result.tools[0].tool.name.as_ref(), "needle");
}

#[tokio::test]
async fn matching_many_cached_subject_catalogs_share_one_inspection_budget() {
    let pool = static_catalog_pool("oauth_000").await;
    let mut configs = Vec::new();
    for index in 0..256 {
        let name = format!("oauth_{index:03}");
        if index > 0 {
            let source_pool = static_catalog_pool(&name).await;
            let mut connections = pool.connections.write().await;
            let mut source = source_pool.connections.write().await;
            connections.insert(
                name.clone(),
                source.remove(&name).expect("fixture connection"),
            );
        }
        move_connection_to_subject_cache_with_tools(
            &pool,
            &name,
            "alice",
            (0..50)
                .map(|tool| test_tool(&format!("ordinary_{index:03}_{tool:02}")))
                .collect(),
        )
        .await;
        configs.push(UpstreamConfig {
            expose_tools: None,
            ..oauth_upstream_config(&name, &["*"])
        });
    }
    let result = pool
        .subject_scoped_upstream_tools_allowed_matching_bounded(
            &configs,
            "alice",
            None,
            10_000,
            &|_, _| false,
            Duration::from_secs(1),
        )
        .await;
    assert_eq!(result.inspected, 10_000);
    assert!(result.incomplete);
    assert!(result.tools.is_empty());
}

fn oauth_upstream_config(name: &str, expose_tools: &[&str]) -> UpstreamConfig {
    UpstreamConfig {
        expose_tools: Some(expose_tools.iter().map(|s| (*s).to_string()).collect()),
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Dynamic,
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        }),
        ..named_test_upstream_config(name)
    }
}

/// Seed the pool so BOTH exposure paths can be queried for the same upstream:
///
/// * the catalog entry carries the compiled `expose_tools` policy, which is what
///   `healthy_tools()` (the non-OAuth path) filters on;
/// * the `(upstream, subject)` connection cache is pre-populated with the same
///   unfiltered tool list, so `subject_scoped_tools()` takes the
///   `acquire_or_connect_subject` fast path and never touches the network.
///
/// The catalog policy is compiled by the same production helper
/// (`resolve_exposure_policy`) that `lazy_upstream_entry` and
/// `replace_catalog_tools` use, not by the test — otherwise the "both paths
/// agree" assertion would only be comparing two things the *test* compiled.
async fn pool_with_both_exposure_paths(upstream: &str, subject: &str) -> Arc<super::UpstreamPool> {
    let pool = static_catalog_pool(upstream).await;
    seed_catalog_entry(&pool, upstream, &EXPOSE_TOOLS).await;
    move_connection_to_subject_cache(&pool, upstream, subject).await;
    pool
}

/// Insert a catalog entry advertising [`DISCOVERED_TOOLS`] with `expose_tools`
/// resolved through the production policy helper.
async fn seed_catalog_entry(pool: &super::UpstreamPool, upstream: &str, expose_tools: &[&str]) {
    let upstream_name: Arc<str> = Arc::from(upstream);
    let mut entry = healthy_in_process_entry(
        Arc::clone(&upstream_name),
        test_upstream_tools(&upstream_name, &DISCOVERED_TOOLS),
    );
    entry.exposure_policy = resolve_exposure_policy(
        upstream,
        Some(expose_tools.iter().map(|s| (*s).to_string()).collect()),
    );
    pool.catalog
        .write()
        .await
        .insert(upstream.to_string(), entry);
}

/// Re-home the fixture's pooled connection into the `(upstream, subject)` cache
/// so `subject_scoped_tools` hits the `acquire_or_connect_subject` fast path.
///
/// The cached tool list is deliberately the **unfiltered** [`DISCOVERED_TOOLS`]:
/// the cache stores what the upstream advertised, and the filter under test runs
/// after the cache read. Seeding it pre-filtered would make every test vacuous.
async fn move_connection_to_subject_cache(
    pool: &super::UpstreamPool,
    upstream: &str,
    subject: &str,
) {
    move_connection_to_subject_cache_with_tools(
        pool,
        upstream,
        subject,
        DISCOVERED_TOOLS.iter().copied().map(test_tool).collect(),
    )
    .await;
}

async fn take_fixture_connection(upstream: &str) -> super::UpstreamConnection {
    let fixture = static_catalog_pool(upstream).await;
    let connection = fixture
        .connections
        .write()
        .await
        .remove(upstream)
        .expect("fixture connection present");
    connection
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort_unstable();
    names
}

#[tokio::test]
async fn subject_catalogs_are_isolated_from_each_other_and_the_global_catalog() {
    let pool = super::UpstreamPool::new();
    let config = oauth_upstream_config("private", &["*"]);
    pool.seed_lazy_upstreams(std::slice::from_ref(&config))
        .await;

    for (subject, tool_name) in [("alice", "alice_private"), ("bob", "bob_private")] {
        let connection = take_fixture_connection("private").await;
        let peer = connection.peer.clone();
        pool.subject_connections.write().await.insert(
            ("private".to_string(), subject.to_string()),
            SubjectScopedConnection {
                _connection: connection,
                peer,
                tools: vec![test_tool(tool_name)],
                last_used: Instant::now(),
            },
        );
    }

    let alice = pool
        .subject_scoped_upstream_tools_allowed(std::slice::from_ref(&config), "alice", None)
        .await;
    let bob = pool
        .subject_scoped_upstream_tools_allowed(std::slice::from_ref(&config), "bob", None)
        .await;

    assert_eq!(alice[0].tool.name.as_ref(), "alice_private");
    assert_eq!(bob[0].tool.name.as_ref(), "bob_private");
    assert!(pool.healthy_tools().await.is_empty());
}

#[tokio::test]
async fn exact_subject_scoped_lookup_projects_only_the_requested_tool() {
    let pool = super::UpstreamPool::new();
    let config = oauth_upstream_config("private", &["*"]);
    let connection = take_fixture_connection("private").await;
    let peer = connection.peer.clone();
    let mut tools = (0..1_000)
        .map(|index| test_tool(&format!("tool_{index:04}")))
        .collect::<Vec<_>>();
    tools[999].input_schema = Arc::new(serde_json::Map::from_iter([(
        "marker".to_string(),
        serde_json::json!({"const": "exact"}),
    )]));
    pool.subject_connections.write().await.insert(
        ("private".to_string(), "alice".to_string()),
        SubjectScopedConnection {
            _connection: connection,
            peer,
            tools,
            last_used: Instant::now(),
        },
    );

    let exact = pool
        .subject_scoped_upstream_tool_allowed(&config, "alice", "tool_0999")
        .await
        .expect("exact exposed tool");
    assert_eq!(exact.tool.name.as_ref(), "tool_0999");
    assert_eq!(
        exact
            .input_schema
            .as_ref()
            .and_then(|schema| schema.get("marker")),
        Some(&serde_json::json!({"const": "exact"}))
    );
}

/// `expose_tools` must hide the same tools on the OAuth subject-scoped path as
/// it does on the catalog-backed path.
///
/// Before this guard, `subject_scoped_tools` returned the upstream's tool list
/// verbatim — `accept()` on the caller side is pagination, not a filter — so an
/// operator who restricted an OAuth upstream to a subset still had every tool
/// advertised to subject-scoped callers, with no error and no log. This test
/// fails without the exposure filter in `subject_scoped_tools`: the
/// subject-scoped list comes back with all three tools, including `delete_repo`.
#[tokio::test]
async fn expose_tools_is_enforced_symmetrically_across_oauth_and_non_oauth_upstreams() {
    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = oauth_upstream_config("github", &EXPOSE_TOOLS);

    let non_oauth: Vec<String> = pool
        .healthy_tools()
        .await
        .into_iter()
        .map(|tool| tool.tool.name.to_string())
        .collect();
    let subject_scoped: Vec<String> = pool
        .subject_scoped_tools(std::slice::from_ref(&config), "alice")
        .await
        .into_iter()
        .flat_map(|(_, tools)| tools)
        .map(|tool| tool.name.to_string())
        .collect();

    assert_eq!(
        sorted(non_oauth.clone()),
        EXPECTED_EXPOSED.map(String::from).to_vec(),
        "catalog-backed exposure is the reference behavior"
    );
    assert_eq!(
        sorted(subject_scoped.clone()),
        sorted(non_oauth),
        "an expose_tools allowlist must hide the same tools on the OAuth path"
    );
    assert!(
        !subject_scoped.contains(&"delete_repo".to_string()),
        "a tool excluded by expose_tools must never reach a subject-scoped caller"
    );
}

/// An unparseable `expose_tools` entry fails closed on the subject-scoped path
/// too — matching `resolve_exposure_policy`'s catalog-path behavior of hiding
/// every tool rather than falling back to exposing everything.
#[tokio::test]
async fn invalid_expose_tools_hides_every_subject_scoped_tool() {
    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = oauth_upstream_config("github", &["   "]);

    let subject_scoped: Vec<(String, Vec<rmcp::model::Tool>)> = pool
        .subject_scoped_tools(std::slice::from_ref(&config), "alice")
        .await;

    assert_eq!(
        subject_scoped.len(),
        1,
        "the upstream is still discovered — it is its tools that are hidden"
    );
    assert!(
        subject_scoped[0].1.is_empty(),
        "an invalid exposure policy must fail closed, not expose everything"
    );
}

/// No `expose_tools` means no allowlist: every discovered tool stays visible, so
/// the filter cannot regress the default configuration.
#[tokio::test]
async fn absent_expose_tools_leaves_subject_scoped_tools_untouched() {
    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = UpstreamConfig {
        expose_tools: None,
        ..oauth_upstream_config("github", &EXPOSE_TOOLS)
    };

    let subject_scoped: Vec<String> = pool
        .subject_scoped_tools(std::slice::from_ref(&config), "alice")
        .await
        .into_iter()
        .flat_map(|(_, tools)| tools)
        .map(|tool| tool.name.to_string())
        .collect();

    assert_eq!(
        sorted(subject_scoped),
        sorted(DISCOVERED_TOOLS.map(String::from).to_vec()),
        "an upstream with no allowlist must keep exposing every discovered tool"
    );
}

/// An explicitly empty `expose_tools = []` hides everything.
///
/// Distinct from both siblings above: `None` (no allowlist) and `Some([])` are a
/// single serde `default` slip apart and mean the *opposite* thing, while the
/// malformed-entry test reaches the same `AllowList(vec![])` value through the
/// error path. Only this test pins `from_optional`'s empty-vec branch.
#[tokio::test]
async fn empty_expose_tools_hides_every_subject_scoped_tool() {
    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = oauth_upstream_config("github", &[]);

    let subject_scoped = pool
        .subject_scoped_tools(std::slice::from_ref(&config), "alice")
        .await;

    assert_eq!(subject_scoped.len(), 1, "the upstream is still discovered");
    assert!(
        subject_scoped[0].1.is_empty(),
        "an explicitly empty allowlist means expose nothing, not expose everything"
    );
}

/// Each upstream must be filtered by *its own* policy.
///
/// `subject_scoped_tools` fans out over `FuturesUnordered`, so results arrive in
/// nondeterministic order. The current code is correct by construction — the
/// policy travels through the result tuple alongside its own config — but that
/// is exactly the invariant a refactor breaks (hoisting the resolve out of the
/// loop, or collecting policies into a `Vec` indexed by completion order). A
/// single-upstream test cannot detect cross-contamination; this one can.
///
/// It also pins the `config.oauth.is_some()` filter: the non-OAuth upstream in
/// the slice must be skipped entirely rather than double-listed alongside the
/// catalog path.
#[tokio::test]
async fn each_upstream_is_filtered_by_its_own_expose_tools() {
    let pool = pool_with_both_exposure_paths("strict", "alice").await;
    seed_catalog_entry(&pool, "open", &["*"]).await;
    let open_pool = static_catalog_pool("open").await;
    {
        let mut connections = pool.connections.write().await;
        let mut source = open_pool.connections.write().await;
        if let Some(connection) = source.remove("open") {
            connections.insert("open".to_string(), connection);
        }
    }
    move_connection_to_subject_cache(&pool, "open", "alice").await;

    let configs = vec![
        oauth_upstream_config("strict", &EXPOSE_TOOLS),
        UpstreamConfig {
            expose_tools: None,
            ..oauth_upstream_config("open", &EXPOSE_TOOLS)
        },
        // No `oauth` block — must be skipped, not listed.
        named_test_upstream_config("plain"),
    ];

    let by_upstream: std::collections::BTreeMap<String, Vec<String>> = pool
        .subject_scoped_tools(&configs, "alice")
        .await
        .into_iter()
        .map(|(name, tools)| {
            (
                name,
                sorted(
                    tools
                        .into_iter()
                        .map(|tool| tool.name.to_string())
                        .collect(),
                ),
            )
        })
        .collect();

    assert_eq!(
        by_upstream.get("strict").map(Vec::as_slice),
        Some(EXPECTED_EXPOSED.map(String::from).to_vec().as_slice()),
        "the restricted upstream keeps its own allowlist"
    );
    assert_eq!(
        by_upstream.get("open").cloned(),
        Some(sorted(DISCOVERED_TOOLS.map(String::from).to_vec())),
        "an unrestricted sibling must not inherit the restricted upstream's allowlist"
    );
    assert!(
        !by_upstream.contains_key("plain"),
        "a non-OAuth upstream must not be listed by the subject-scoped path"
    );
}

#[tokio::test]
async fn subject_scoped_tools_have_stable_upstream_and_tool_order() {
    let pool = pool_with_both_exposure_paths("zeta", "alice").await;
    seed_catalog_entry(&pool, "alpha", &["*"]).await;
    let alpha_pool = static_catalog_pool("alpha").await;
    {
        let mut connections = pool.connections.write().await;
        let mut source = alpha_pool.connections.write().await;
        if let Some(connection) = source.remove("alpha") {
            connections.insert("alpha".to_string(), connection);
        }
    }
    move_connection_to_subject_cache(&pool, "alpha", "alice").await;
    let configs = vec![
        oauth_upstream_config("zeta", &["*"]),
        oauth_upstream_config("alpha", &["*"]),
    ];

    let listed = pool.subject_scoped_tools(&configs, "alice").await;
    let upstreams = listed
        .iter()
        .map(|(upstream, _)| upstream.as_str())
        .collect::<Vec<_>>();

    assert_eq!(upstreams, vec!["alpha", "zeta"]);
    for (_, tools) in listed {
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert!(names.is_sorted());
    }
}

#[tokio::test]
async fn bounded_subject_scoped_tools_select_stably_across_over_cap_upstreams() {
    const TOOLS_PER_UPSTREAM: usize = 600;
    let pool = static_catalog_pool("zeta").await;
    let zeta_tools = (0..TOOLS_PER_UPSTREAM)
        .rev()
        .map(|index| test_tool(&format!("zeta_tool_{index:04}")))
        .collect();
    move_connection_to_subject_cache_with_tools(&pool, "zeta", "alice", zeta_tools).await;

    let alpha_pool = static_catalog_pool("alpha").await;
    {
        let mut connections = pool.connections.write().await;
        let mut source = alpha_pool.connections.write().await;
        connections.insert(
            "alpha".to_string(),
            source.remove("alpha").expect("alpha fixture connection"),
        );
    }
    let alpha_tools = (0..TOOLS_PER_UPSTREAM)
        .rev()
        .map(|index| test_tool(&format!("alpha_tool_{index:04}")))
        .collect();
    move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", alpha_tools).await;

    let alpha = UpstreamConfig {
        expose_tools: None,
        ..oauth_upstream_config("alpha", &["*"])
    };
    let zeta = UpstreamConfig {
        expose_tools: None,
        ..oauth_upstream_config("zeta", &["*"])
    };
    let first = pool
        .subject_scoped_tools_bounded(&[zeta.clone(), alpha.clone()], "alice", 1_000)
        .await;
    let second = pool
        .subject_scoped_tools_bounded(&[alpha, zeta], "alice", 1_000)
        .await;

    let flatten = |listed: Vec<(String, Vec<rmcp::model::Tool>)>| {
        listed
            .into_iter()
            .flat_map(|(upstream, tools)| {
                tools
                    .into_iter()
                    .map(move |tool| format!("{upstream}::{}", tool.name))
            })
            .collect::<Vec<_>>()
    };
    let first = flatten(first);
    let second = flatten(second);

    assert_eq!(
        first, second,
        "completion and config order must not change selection"
    );
    assert_eq!(first.len(), 1_000);
    assert_eq!(
        first.first().map(String::as_str),
        Some("alpha::alpha_tool_0000")
    );
    assert_eq!(
        first.get(599).map(String::as_str),
        Some("alpha::alpha_tool_0599")
    );
    assert_eq!(
        first.get(600).map(String::as_str),
        Some("zeta::zeta_tool_0000")
    );
    assert_eq!(
        first.last().map(String::as_str),
        Some("zeta::zeta_tool_0399")
    );
}

/// Hidden means uncallable, not merely unadvertised.
///
/// This is the OAuth twin of `hidden_upstream_tools_cannot_be_called_directly`
/// (`pool/tools.rs`), which pins the same property for the catalog path. It
/// covers both halves of the guarantee:
///
/// 1. **Routing** — the owner-resolution scan that
///    `crates/labby/src/mcp/call_tool_upstream.rs` performs over
///    `subject_scoped_tools` finds no owner for a hidden tool.
/// 2. **Execution** — the pool's own subject-scoped call primitive refuses the
///    call outright, so the guarantee does not depend on that one caller. This
///    half is what closes the `pre_resolved_oauth_config` branch, which resolves
///    its owner from the catalog and never consults `subject_scoped_tools`.
#[tokio::test]
async fn hidden_subject_scoped_tools_cannot_be_called() {
    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = oauth_upstream_config("github", &EXPOSE_TOOLS);

    // 1. Routing: mirrors the owner-resolution loop in call_tool_upstream.rs.
    let owner_of = |tool_name: &'static str| {
        let pool = Arc::clone(&pool);
        let config = config.clone();
        async move {
            pool.subject_scoped_tools(std::slice::from_ref(&config), "alice")
                .await
                .into_iter()
                .find(|(_, tools)| tools.iter().any(|tool| tool.name.as_ref() == tool_name))
                .map(|(upstream, _)| upstream)
        }
    };
    assert_eq!(
        owner_of("search_repos").await.as_deref(),
        Some("github"),
        "an exposed tool must still resolve to its owning upstream"
    );
    assert_eq!(
        owner_of("delete_repo").await,
        None,
        "a hidden tool must resolve to no upstream, so the proxy never routes it"
    );

    // 2. Execution: the pool primitive refuses regardless of how it was reached.
    let call = |tool_name: &'static str| {
        let pool = Arc::clone(&pool);
        let config = config.clone();
        async move {
            pool.subject_scoped_call_tool(
                &config,
                "alice",
                rmcp::model::CallToolRequestParams::new(tool_name),
            )
            .await
        }
    };
    let hidden = call("delete_repo").await;
    assert!(
        hidden
            .as_ref()
            .is_err_and(|error| error.contains("does not expose tool `delete_repo`")),
        "a hidden tool must be refused by the pool itself, got {hidden:?}"
    );
    // The exposed tool must get *past* the guard. Whether the fixture upstream
    // then answers it is not this test's business — asserting `is_ok()` would
    // couple the exposure guard to the mock server's tool table.
    let exposed = call("search_repos").await;
    assert!(
        !exposed
            .as_ref()
            .is_err_and(|error| error.contains("does not expose tool")),
        "an exposed tool must not be blocked by the exposure guard, got {exposed:?}"
    );
}

/// A refused call must surface as `unknown_tool`, not as a retryable error.
///
/// The classified call path is what the MCP proxy actually uses
/// (`call_tool_upstream.rs` → `subject_scoped_call_tool_once_classified`), and
/// the failure *class* is load-bearing: `mcp_error_data_kind` maps
/// `METHOD_NOT_FOUND` to the `unknown_tool` stable kind, whose recovery contract
/// tells the agent to rediscover. A `Transport`-class refusal would instead
/// surface as `upstream_error` and instruct the agent to retry a denial that can
/// never succeed.
#[tokio::test]
async fn a_hidden_tool_is_refused_as_unknown_tool_not_as_a_retryable_error() {
    use super::super::tool_error::mcp_error_data_kind;
    use super::capability_call::CapabilityCallError;

    let pool = pool_with_both_exposure_paths("github", "alice").await;
    let config = oauth_upstream_config("github", &EXPOSE_TOOLS);

    let proxy_error = pool
        .subject_scoped_call_tool_once_classified(
            &config,
            "alice",
            rmcp::model::CallToolRequestParams::new("delete_repo"),
            None,
        )
        .await
        .expect_err("a hidden tool must not be callable");
    let code_mode_error = pool
        .subject_scoped_call_tool_classified(
            &config,
            "alice",
            rmcp::model::CallToolRequestParams::new("delete_repo"),
        )
        .await
        .expect_err("Code Mode must enforce the same exposure policy");

    for error in [proxy_error, code_mode_error] {
        let CapabilityCallError::Mcp { data, message } = &error else {
            panic!("expected an Mcp-class refusal, got {error:?}");
        };
        // Pin that the refusal came from the exposure guard specifically.
        // Without this the test would also pass if the guard were removed and
        // the fixture upstream simply answered "no such tool" — a false green.
        assert!(
            message.contains("does not expose tool `delete_repo`"),
            "the refusal must come from the exposure guard, got {message:?}"
        );
        assert_eq!(
            mcp_error_data_kind(data),
            "unknown_tool",
            "a hidden tool must be indistinguishable from one the upstream never advertised"
        );
    }
}
