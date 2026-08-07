//! Regression tests for `expose_tools` enforcement symmetry.
//!
//! An operator's `expose_tools` allowlist must hide the same tools regardless of
//! how the upstream is authenticated. The catalog-backed (non-OAuth) path
//! filters through `UpstreamEntry::exposure_policy`; the OAuth subject-scoped
//! path has no catalog entry to consult and resolves the same policy from the
//! live `UpstreamConfig`. These tests pin both halves together so the two can
//! never drift again.

use std::sync::Arc;
use std::time::Instant;

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::super::types::ToolExposurePolicy;
use super::SubjectScopedConnection;
use super::entries::healthy_in_process_entry;
use super::testsupport::*;

/// The tools every fixture upstream advertises, and the allowlist applied to it.
const DISCOVERED_TOOLS: [&str; 3] = ["search_repos", "github_create_issue", "delete_repo"];
const EXPOSE_TOOLS: [&str; 2] = ["search_repos", "github_*"];
const EXPECTED_EXPOSED: [&str; 2] = ["github_create_issue", "search_repos"];

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
async fn pool_with_both_exposure_paths(upstream: &str, subject: &str) -> Arc<super::UpstreamPool> {
    let pool = static_catalog_pool(upstream).await;
    let upstream_name: Arc<str> = Arc::from(upstream);

    let mut entry = healthy_in_process_entry(
        Arc::clone(&upstream_name),
        test_upstream_tools(&upstream_name, &DISCOVERED_TOOLS),
    );
    entry.exposure_policy =
        ToolExposurePolicy::from_patterns(EXPOSE_TOOLS.iter().map(|s| (*s).to_string()).collect())
            .expect("policy compiles");
    pool.catalog
        .write()
        .await
        .insert(upstream.to_string(), entry);

    // `UpstreamConnection` implements `Drop`, so the whole value has to be moved
    // out of the pool rather than having its fields taken individually.
    let peer = pool
        .connections
        .read()
        .await
        .get(upstream)
        .expect("fixture connection present")
        .peer
        .clone();
    let connection = pool
        .connections
        .write()
        .await
        .remove(upstream)
        .expect("fixture connection present");
    pool.subject_connections.write().await.insert(
        (upstream.to_string(), subject.to_string()),
        SubjectScopedConnection {
            _connection: connection,
            peer,
            tools: DISCOVERED_TOOLS.iter().copied().map(test_tool).collect(),
            last_used: Instant::now(),
        },
    );

    pool
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort_unstable();
    names
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
