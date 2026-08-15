//! Regression tests for verbatim relay of upstream `ToolAnnotations`.
//!
//! Labby advertises its own tools' safety hints (issue #212), and in a
//! labby → labby chain the next hop derives its `destructive` gate from the
//! hints it receives. That makes *not mangling* an upstream's annotations a
//! load-bearing property, not a cosmetic one: filling in a missing hint, or
//! dropping one we do not understand, changes another gateway's authorization
//! decision.
//!
//! `docs/surfaces/MCP.md` states the contract — annotations pass through
//! verbatim "including `title`, unknown or future fields, and the absence of
//! the block", on every listing path. The aggregated raw path is covered in
//! `crates/labby/src/mcp/handlers_tools/tests.rs`. This file covers the two
//! paths that file cannot reach:
//!
//! 1. **The OAuth subject-scoped path.** It has no `UpstreamEntry` and never
//!    builds an `UpstreamTool`; it clones `rmcp::model::Tool` values straight
//!    out of the per-subject connection cache. It is therefore structurally
//!    incapable of inheriting the aggregated path's behavior, and was called
//!    the most regression-prone path in the design package.
//! 2. **`cached_upstream_tool`.** The single chokepoint where a relayed tool is
//!    cached. Its fail-closed `destructive` derivation is already covered by
//!    `helpers.rs`; what is pinned here is the *other* half — that deriving
//!    that gate does not disturb the annotations being relayed.

use std::sync::Arc;

use super::testsupport::*;

/// A fully-populated annotation block, including `title`, which is the field
/// most likely to be dropped by a descriptor rebuild.
fn full_annotations() -> rmcp::model::ToolAnnotations {
    let mut annotations = rmcp::model::ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false);
    annotations.title = Some("Reviewed upstream title".to_string());
    annotations
}

fn annotated_tool(
    name: &str,
    annotations: Option<rmcp::model::ToolAnnotations>,
) -> rmcp::model::Tool {
    let mut tool = test_tool(name);
    tool.annotations = annotations;
    tool
}

/// Read the subject-scoped listing for a single seeded upstream.
async fn subject_listing(tools: Vec<rmcp::model::Tool>) -> Vec<rmcp::model::Tool> {
    let upstream = "annotated";
    let subject = "alice";
    let pool = static_catalog_pool(upstream).await;
    move_connection_to_subject_cache_with_tools(&pool, upstream, subject, tools).await;
    let config = labby_runtime::gateway_config::UpstreamConfig {
        oauth: Some(labby_runtime::gateway_config::UpstreamOauthConfig {
            mode: labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce,
            registration: labby_runtime::gateway_config::UpstreamOauthRegistration::Dynamic,
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        }),
        ..named_test_upstream_config(upstream)
    };
    let listed = pool
        .cached_subject_scoped_tools_bounded(std::slice::from_ref(&config), subject, 64)
        .await;
    listed
        .into_iter()
        .flat_map(|(_upstream, tools)| tools)
        .collect()
}

#[tokio::test]
async fn subject_scoped_listing_relays_a_full_annotation_block_verbatim() {
    let expected = full_annotations();
    let listed = subject_listing(vec![annotated_tool("read", Some(expected.clone()))]).await;

    let tool = listed
        .iter()
        .find(|tool| tool.name.as_ref() == "read")
        .expect("seeded tool is listed");
    assert_eq!(
        tool.annotations.as_ref(),
        Some(&expected),
        "the OAuth subject-scoped path must relay annotations byte-identically, \
         including `title`"
    );
}

/// A partial block must stay partial. Filling in an absent hint would invent a
/// claim the upstream never made — and in a labby → labby chain the next hop
/// would gate on our invention.
#[tokio::test]
async fn subject_scoped_listing_does_not_fill_in_absent_hints() {
    let mut partial = rmcp::model::ToolAnnotations::new().read_only(true);
    partial.title = Some("Only readOnly is asserted".to_string());
    let listed = subject_listing(vec![annotated_tool("partial", Some(partial.clone()))]).await;

    let annotations = listed
        .iter()
        .find(|tool| tool.name.as_ref() == "partial")
        .and_then(|tool| tool.annotations.as_ref())
        .expect("annotations survive");
    assert_eq!(annotations, &partial);
    assert_eq!(annotations.read_only_hint, Some(true));
    for (field, value) in [
        ("destructiveHint", annotations.destructive_hint),
        ("idempotentHint", annotations.idempotent_hint),
        ("openWorldHint", annotations.open_world_hint),
    ] {
        assert_eq!(value, None, "{field} must stay absent, not be defaulted");
    }
}

/// Absence of the block is itself information the contract preserves.
#[tokio::test]
async fn subject_scoped_listing_preserves_absent_annotations() {
    let listed = subject_listing(vec![annotated_tool("bare", None)]).await;

    let tool = listed
        .iter()
        .find(|tool| tool.name.as_ref() == "bare")
        .expect("seeded tool is listed");
    assert!(
        tool.annotations.is_none(),
        "an unannotated upstream tool must not gain a synthesized block"
    );
}

/// Mixed set: annotated and unannotated tools coexist without cross-contamination.
#[tokio::test]
async fn subject_scoped_listing_keeps_per_tool_annotations_separate() {
    let expected = full_annotations();
    let listed = subject_listing(vec![
        annotated_tool("annotated", Some(expected.clone())),
        annotated_tool("bare", None),
    ])
    .await;

    assert_eq!(listed.len(), 2, "both tools listed");
    for tool in &listed {
        match tool.name.as_ref() {
            "annotated" => assert_eq!(tool.annotations.as_ref(), Some(&expected)),
            "bare" => assert!(tool.annotations.is_none()),
            other => panic!("unexpected tool `{other}`"),
        }
    }
}

/// `cached_upstream_tool` derives the gateway-side `destructive` gate from the
/// annotations. That derivation must not disturb the annotations themselves —
/// the derived value is internal and never reaches the wire, while the block
/// is relayed to clients untouched.
///
/// The fail-closed derivation itself is covered by
/// `cached_upstream_tool_fails_closed_without_destructive_annotations` and
/// `..._honors_explicit_non_destructive_hints` in `helpers.rs`; this asserts
/// only the relay half, so the two do not duplicate each other.
#[tokio::test]
async fn cached_upstream_tool_relays_annotations_untouched() {
    let upstream_name: Arc<str> = Arc::from("annotated");
    let expected = full_annotations();

    let (_name, cached) = super::helpers::cached_upstream_tool(
        annotated_tool("read", Some(expected.clone())),
        &upstream_name,
    );
    assert_eq!(
        cached.tool.annotations.as_ref(),
        Some(&expected),
        "caching must not rewrite the relayed annotations"
    );
    // Sanity: the derivation did read them (readOnly + non-destructive => not
    // destructive), so the assertion above is not passing over a no-op path.
    assert!(!cached.destructive);

    let (_name, bare) =
        super::helpers::cached_upstream_tool(annotated_tool("bare", None), &upstream_name);
    assert!(
        bare.tool.annotations.is_none(),
        "absence must survive caching too"
    );
    assert!(
        bare.destructive,
        "missing annotations still fail closed on the gate"
    );
}
