//! Regression tests for `expose_resources` enforcement.
//!
//! `expose_resources` was parsed, persisted, patchable and projected to the UI,
//! but no code path ever consulted it — an operator's allowlist restricted
//! nothing. These tests pin both halves of the fix:
//!
//! * the **list** is filtered (catalog-backed and OAuth subject-scoped), and
//! * the **read** is filtered too, because a list-only filter is a bypass: an
//!   excluded resource would stay directly readable by anyone holding its URI.

use std::sync::Arc;
use std::time::Instant;

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::SubjectScopedConnection;
use super::UpstreamPool;
use super::entries::resolve_resource_exposure_policy;
use super::testsupport::*;

/// `StaticCatalogServer` advertises exactly these two resources.
const EXPOSED_URI: &str = "file:///tmp/upstream-one";
const HIDDEN_URI: &str = "lab://upstream/old-name/file:///tmp/upstream-two";

fn gateway_uri(upstream: &str, bare_uri: &str) -> String {
    format!("lab://upstream/{upstream}/{bare_uri}")
}

fn oauth_upstream_config(name: &str, expose_resources: Option<Vec<&str>>) -> UpstreamConfig {
    UpstreamConfig {
        proxy_resources: true,
        expose_resources: expose_resources
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

/// Seed a catalog-backed pool whose entry carries a compiled `expose_resources`
/// policy, exactly as `lazy_upstream_entry`/`discover` would build it.
async fn catalog_pool_with_expose_resources(
    upstream: &str,
    expose_resources: Option<Vec<&str>>,
) -> Arc<UpstreamPool> {
    let pool = static_catalog_pool(upstream).await;
    let policy = resolve_resource_exposure_policy(
        upstream,
        expose_resources.map(|patterns| patterns.into_iter().map(str::to_string).collect()),
    );
    let mut catalog = pool.catalog_write().await;
    catalog
        .get_mut(upstream)
        .expect("fixture catalog entry")
        .resource_exposure_policy = policy;
    drop(catalog);
    pool
}

/// Move the fixture's live connection into the `(upstream, subject)` cache so
/// `acquire_or_connect_subject` takes its fast path and no network is touched.
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

async fn listed_uris(pool: &UpstreamPool) -> Vec<String> {
    pool.list_upstream_resources()
        .await
        .into_iter()
        .map(|resource| resource.uri)
        .collect()
}

/// Without the fix `list_upstream_resources` returns both fixture resources
/// regardless of `expose_resources`.
#[tokio::test]
async fn expose_resources_hides_unlisted_resources_from_the_catalog_listing() {
    let pool = catalog_pool_with_expose_resources("static", Some(vec![EXPOSED_URI])).await;

    let uris = listed_uris(&pool).await;

    assert_eq!(uris, vec![gateway_uri("static", EXPOSED_URI)]);
    assert!(
        !uris.iter().any(|uri| uri.contains("upstream-two")),
        "a resource excluded by expose_resources must not be advertised"
    );
}

/// The read is the gate that matters: filtering the list while leaving
/// `resources/read` open leaves the excluded resource fully reachable by URI.
/// Without the fix this read is forwarded and returns `Some(_)`.
#[tokio::test]
async fn expose_resources_blocks_a_direct_read_of_an_excluded_resource() {
    let pool = catalog_pool_with_expose_resources("static", Some(vec![EXPOSED_URI])).await;

    assert!(
        pool.read_upstream_resource(&gateway_uri("static", HIDDEN_URI))
            .await
            .is_none(),
        "an excluded resource must not be readable by URI — list filtering alone is a bypass"
    );
    assert!(
        pool.read_upstream_resource(&gateway_uri("static", EXPOSED_URI))
            .await
            .is_some(),
        "an allowed resource must still be forwarded to the upstream"
    );
}

/// An unparseable allowlist hides everything rather than degrading to "expose
/// all" — the same fail-closed contract `resolve_exposure_policy` gives tools.
#[tokio::test]
async fn invalid_expose_resources_hides_every_resource() {
    let pool = catalog_pool_with_expose_resources("static", Some(vec!["   "])).await;

    assert!(
        listed_uris(&pool).await.is_empty(),
        "an invalid exposure policy must fail closed, not expose everything"
    );
    assert!(
        pool.read_upstream_resource(&gateway_uri("static", EXPOSED_URI))
            .await
            .is_none(),
        "fail-closed must cover the read path too"
    );
}

/// No allowlist is still no restriction.
#[tokio::test]
async fn absent_expose_resources_is_a_no_op() {
    let pool = catalog_pool_with_expose_resources("static", None).await;

    let uris = listed_uris(&pool).await;

    assert_eq!(uris.len(), 2, "both fixture resources stay visible");
    assert!(
        pool.read_upstream_resource(&gateway_uri("static", HIDDEN_URI))
            .await
            .is_some(),
        "every resource stays readable when no allowlist is configured"
    );
}

/// The OAuth subject-scoped read has no catalog entry to consult, so it
/// resolves the policy from the live config. Without the fix the excluded
/// resource is read successfully over the subject connection.
#[tokio::test]
async fn expose_resources_blocks_a_subject_scoped_read_of_an_excluded_resource() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec![EXPOSED_URI]));

    let blocked = pool
        .subject_scoped_read_resource(&config, "alice", &gateway_uri("static", HIDDEN_URI))
        .await
        .expect_err("an excluded resource must not be readable over the subject connection");

    assert!(
        blocked.contains("not exposed"),
        "unexpected subject-scoped read error: {blocked}"
    );
}

/// Fail-closed applies to the subject-scoped read as well.
#[tokio::test]
async fn invalid_expose_resources_blocks_every_subject_scoped_read() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", Some(vec!["   "]));

    let blocked = pool
        .subject_scoped_read_resource(&config, "alice", &gateway_uri("static", EXPOSED_URI))
        .await
        .expect_err("an invalid allowlist must hide every resource");

    assert!(
        blocked.contains("not exposed"),
        "unexpected subject-scoped read error: {blocked}"
    );
}

/// …and an absent allowlist leaves the subject-scoped read untouched.
#[tokio::test]
async fn absent_expose_resources_leaves_subject_scoped_reads_alone() {
    let pool = static_catalog_pool("static").await;
    seed_subject_connection(&pool, "static", "alice").await;
    let config = oauth_upstream_config("static", None);

    // `StaticCatalogServer` has no `read_resource` handler, so the upstream
    // rejects the forwarded request. The point is that it *was* forwarded:
    // the error comes from the upstream, not from the exposure gate.
    let error = pool
        .subject_scoped_read_resource(&config, "alice", &gateway_uri("static", EXPOSED_URI))
        .await
        .expect_err("fixture upstream implements no read_resource handler");

    assert!(
        !error.contains("not exposed"),
        "the read must reach the upstream when no allowlist is configured: {error}"
    );
}
