//! Regression tests for `expose_prompts` enforcement.
//!
//! The prompt half of the same gap `resources_exposure_tests.rs` covers:
//! `expose_prompts` was persisted and shown in the UI but consulted nowhere, so
//! an operator's allowlist restricted nothing. Both the list and the direct
//! `prompts/get` are pinned here — a list-only filter would leave the excluded
//! prompt fetchable by name, which is the prompt-side twin of the
//! `resources/read` bypass.

use std::sync::Arc;
use std::time::Instant;

use rmcp::model::GetPromptRequestParams;

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::SubjectScopedConnection;
use super::UpstreamPool;
use super::entries::resolve_prompt_exposure_policy;
use super::testsupport::*;

/// `StaticCatalogServer` advertises exactly these two prompts (bare names).
const EXPOSED_PROMPT: &str = "upstream.prompt.one";
const HIDDEN_PROMPT: &str = "upstream.prompt.two";

fn namespaced(upstream: &str, prompt: &str) -> String {
    format!("{upstream}/{prompt}")
}

fn oauth_upstream_config(name: &str, expose_prompts: Option<Vec<&str>>) -> UpstreamConfig {
    UpstreamConfig {
        proxy_prompts: true,
        expose_prompts: expose_prompts
            .map(|patterns| patterns.into_iter().map(str::to_string).collect()),
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

async fn catalog_pool_with_expose_prompts(
    upstream: &str,
    expose_prompts: Option<Vec<&str>>,
) -> Arc<UpstreamPool> {
    let pool = static_catalog_pool(upstream).await;
    let policy = resolve_prompt_exposure_policy(
        upstream,
        expose_prompts.map(|patterns| patterns.into_iter().map(str::to_string).collect()),
    );
    let mut catalog = pool.catalog_write().await;
    catalog
        .get_mut(upstream)
        .expect("fixture catalog entry")
        .prompt_exposure_policy = policy;
    drop(catalog);
    pool
}

/// Move the fixture connection into the `(upstream, subject)` cache so
/// `acquire_or_connect_subject` hits its fast path — no network involved.
async fn seed_subject_connection(pool: &UpstreamPool, upstream: &str, subject: &str) {
    let peer = pool
        .connections
        .read()
        .await
        .get(upstream)
        .expect("fixture connection present")
        .peer
        .clone();
    // `UpstreamConnection` implements `Drop`, so the whole value has to move.
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
            tools: Vec::new(),
            last_used: Instant::now(),
        },
    );
}

async fn listed_prompt_names(pool: &UpstreamPool) -> Vec<String> {
    pool.list_upstream_prompts(&[])
        .await
        .into_iter()
        .map(|prompt| prompt.name.to_string())
        .collect()
}

/// Without the fix both fixture prompts are listed regardless of the allowlist.
#[tokio::test]
async fn expose_prompts_hides_unlisted_prompts_from_the_catalog_listing() {
    let pool = catalog_pool_with_expose_prompts("static", Some(vec![EXPOSED_PROMPT])).await;

    assert_eq!(
        listed_prompt_names(&pool).await,
        vec![namespaced("static", EXPOSED_PROMPT)]
    );
}

/// Operators copy allowlist entries out of `gateway.discovered_prompts`, which
/// reports the `{upstream}/{name}` namespaced spelling. That spelling must work
/// as well as the bare name the upstream itself advertises.
#[tokio::test]
async fn expose_prompts_accepts_the_namespaced_spelling_shown_in_the_admin_ui() {
    let pool = catalog_pool_with_expose_prompts(
        "static",
        Some(vec![&namespaced("static", EXPOSED_PROMPT)]),
    )
    .await;

    assert_eq!(
        listed_prompt_names(&pool).await,
        vec![namespaced("static", EXPOSED_PROMPT)]
    );
}

/// The direct fetch is the gate that matters — an excluded prompt must not be
/// retrievable by name. Without the fix this call is forwarded and succeeds.
#[tokio::test]
async fn expose_prompts_blocks_a_direct_get_of_an_excluded_prompt() {
    let pool = catalog_pool_with_expose_prompts("static", Some(vec![EXPOSED_PROMPT])).await;

    let blocked = pool
        .get_prompt(
            "static",
            GetPromptRequestParams::new(namespaced("static", HIDDEN_PROMPT)),
        )
        .await
        .expect("upstream stays connected")
        .expect_err("an excluded prompt must not be fetchable by name");
    assert!(
        blocked.contains("not exposed"),
        "unexpected prompt get error: {blocked}"
    );

    pool.get_prompt(
        "static",
        GetPromptRequestParams::new(namespaced("static", EXPOSED_PROMPT)),
    )
    .await
    .expect("upstream stays connected")
    .expect("an allowed prompt must still be forwarded");
}

/// An unparseable allowlist hides every prompt rather than degrading to
/// "expose all".
#[tokio::test]
async fn invalid_expose_prompts_hides_every_prompt() {
    let pool = catalog_pool_with_expose_prompts("static", Some(vec!["   "])).await;

    assert!(
        listed_prompt_names(&pool).await.is_empty(),
        "an invalid exposure policy must fail closed, not expose everything"
    );
    let blocked = pool
        .get_prompt(
            "static",
            GetPromptRequestParams::new(namespaced("static", EXPOSED_PROMPT)),
        )
        .await
        .expect("upstream stays connected")
        .expect_err("fail-closed must cover the fetch path too");
    assert!(
        blocked.contains("not exposed"),
        "unexpected prompt get error: {blocked}"
    );
}

/// No allowlist is still no restriction.
#[tokio::test]
async fn absent_expose_prompts_is_a_no_op() {
    let pool = catalog_pool_with_expose_prompts("static", None).await;

    assert_eq!(
        listed_prompt_names(&pool).await,
        vec![
            namespaced("static", EXPOSED_PROMPT),
            namespaced("static", HIDDEN_PROMPT),
        ]
    );
    pool.get_prompt(
        "static",
        GetPromptRequestParams::new(namespaced("static", HIDDEN_PROMPT)),
    )
    .await
    .expect("upstream stays connected")
    .expect("every prompt stays fetchable when no allowlist is configured");
}

/// The OAuth subject-scoped list has no catalog entry to consult, so it
/// resolves the policy from the live config. Without the fix the excluded
/// prompt is advertised to the subject-scoped caller.
#[tokio::test]
async fn expose_prompts_filters_the_subject_scoped_listing() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec![EXPOSED_PROMPT]));

    let names: Vec<String> = pool
        .subject_scoped_prompts(std::slice::from_ref(&config), "alice", &[])
        .await
        .into_iter()
        .map(|prompt| prompt.name.to_string())
        .collect();

    assert_eq!(names, vec![namespaced("static", EXPOSED_PROMPT)]);
}

/// …and the subject-scoped fetch is gated too, so the filtered list is not
/// merely cosmetic.
#[tokio::test]
async fn expose_prompts_blocks_a_subject_scoped_get_of_an_excluded_prompt() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec![EXPOSED_PROMPT]));

    let blocked = pool
        .subject_scoped_get_prompt(
            &config,
            "alice",
            GetPromptRequestParams::new(namespaced("static", HIDDEN_PROMPT)),
        )
        .await
        .expect_err("an excluded prompt must not be fetchable over the subject connection");
    assert!(
        blocked.contains("not exposed"),
        "unexpected subject-scoped prompt get error: {blocked}"
    );

    pool.subject_scoped_get_prompt(
        &config,
        "alice",
        GetPromptRequestParams::new(namespaced("static", EXPOSED_PROMPT)),
    )
    .await
    .expect("an allowed prompt must still be forwarded");
}

/// Owner resolution must not point at a hidden prompt either — otherwise the
/// caller routes a fetch at something it is not allowed to see.
#[tokio::test]
async fn expose_prompts_hides_excluded_prompts_from_subject_scoped_owner_lookup() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec![EXPOSED_PROMPT]));
    let configs = std::slice::from_ref(&config);

    assert_eq!(
        pool.subject_scoped_prompt_owner(configs, "alice", &namespaced("static", EXPOSED_PROMPT))
            .await
            .as_deref(),
        Some("static")
    );
    assert_eq!(
        pool.subject_scoped_prompt_owner(configs, "alice", &namespaced("static", HIDDEN_PROMPT))
            .await,
        None,
        "an excluded prompt must not resolve an owner"
    );
}

/// Fail-closed applies to the subject-scoped paths as well.
#[tokio::test]
async fn invalid_expose_prompts_hides_every_subject_scoped_prompt() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec!["   "]));

    assert!(
        pool.subject_scoped_prompts(std::slice::from_ref(&config), "alice", &[])
            .await
            .is_empty(),
        "an invalid exposure policy must fail closed, not expose everything"
    );
}

/// …and an absent allowlist leaves the subject-scoped paths untouched.
#[tokio::test]
async fn absent_expose_prompts_leaves_subject_scoped_prompts_alone() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", None);

    assert_eq!(
        pool.subject_scoped_prompts(std::slice::from_ref(&config), "alice", &[])
            .await
            .len(),
        2
    );
    pool.subject_scoped_get_prompt(
        &config,
        "alice",
        GetPromptRequestParams::new(namespaced("static", HIDDEN_PROMPT)),
    )
    .await
    .expect("every prompt stays fetchable when no allowlist is configured");
}

/// Holding the fleet-wide catalog permit must stop even a cache-hot subject
/// listing from entering its acquisition/listing job. This directly pins the
/// subject prompt path to the shared global fan-out gate.
#[tokio::test]
async fn subject_prompt_listing_waits_for_global_catalog_fanout_permit() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let permits = pool.catalog_fanout_semaphore.available_permits() as u32;
    let held = Arc::clone(&pool.catalog_fanout_semaphore)
        .acquire_many_owned(permits)
        .await
        .expect("hold every global permit");
    let config = oauth_upstream_config("static", None);
    let deadline_at = tokio::time::Instant::now() + std::time::Duration::from_millis(25);

    let prompts = pool
        .subject_scoped_prompts_until(std::slice::from_ref(&config), "alice", &[], deadline_at)
        .await;

    assert!(prompts.is_empty());
    drop(held);
}
