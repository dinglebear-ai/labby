//! Regressions for upstream skills enumeration and retrieval (SEP-2640).
//!
//! The mock upstream here answers `skills/list` and `skills/get` through
//! `on_custom_request`, which is how a real skills-capable server answers them:
//! rmcp has no typed skills methods, so both sides ride the custom-request
//! catch-all.
//!
//! What these pin down, in rough order of how badly it would hurt to get wrong:
//! a partial walk is never returned as if it were complete; budgets stop the
//! walk *before* the next request rather than after accumulating everything;
//! one malformed skill never sinks the upstream; and `-32602` from `skills/get`
//! is the one answer that means "not a skill", not a transport failure.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::model::{
    CustomRequest, CustomResult, ErrorCode, ErrorData, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::{Value, json};

use labby_runtime::skills::digest::ResourceDigest;
use labby_runtime::skills::{SkillRejection, limits};

use super::testsupport::*;

/// Build a well-formed entry whose manifest lists its own `SKILL.md`.
fn entry(origin: &str, name: &str) -> Value {
    let uri = format!("skill://{origin}/{name}/SKILL.md");
    json!({
        "uri": uri,
        "frontmatter": { "name": name, "description": "a test skill" },
        "resources": [
            { "uri": uri, "digest": ResourceDigest::of_bytes(b"body").to_wire() }
        ]
    })
}

/// A skills-capable upstream whose `skills/list` behavior is scripted per test.
#[derive(Clone)]
struct SkillsServer {
    /// One JSON result per `skills/list` call, in order. The last is repeated
    /// if the client asks for more pages than were scripted.
    pages: Arc<Vec<Value>>,
    list_calls: Arc<AtomicUsize>,
    get_calls: Arc<AtomicUsize>,
    /// When set, `skills/get` answers with this entry; otherwise -32602.
    get_entry: Arc<Option<Value>>,
    /// When false, the handshake omits the skills extension entirely.
    declares_extension: bool,
}

impl SkillsServer {
    fn new(pages: Vec<Value>) -> Self {
        Self {
            pages: Arc::new(pages),
            list_calls: Arc::new(AtomicUsize::new(0)),
            get_calls: Arc::new(AtomicUsize::new(0)),
            get_entry: Arc::new(None),
            declares_extension: true,
        }
    }

    fn with_get(mut self, entry: Value) -> Self {
        self.get_entry = Arc::new(Some(entry));
        self
    }

    fn without_extension(mut self) -> Self {
        self.declares_extension = false;
        self
    }
}

impl ServerHandler for SkillsServer {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        if self.declares_extension {
            let mut extensions = rmcp::model::ExtensionCapabilities::new();
            extensions.insert(
                labby_runtime::skills::wire::SKILLS_EXTENSION_KEY.to_string(),
                serde_json::Map::new(),
            );
            capabilities.extensions = Some(extensions);
        }
        ServerInfo::new(capabilities)
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        match request.method.as_str() {
            "skills/list" => {
                let index = self.list_calls.fetch_add(1, Ordering::SeqCst);
                let page = self
                    .pages
                    .get(index)
                    .or_else(|| self.pages.last())
                    .cloned()
                    .unwrap_or_else(|| json!({ "skills": [] }));
                Ok(CustomResult::new(page))
            }
            "skills/get" => {
                self.get_calls.fetch_add(1, Ordering::SeqCst);
                match self.get_entry.as_ref() {
                    Some(entry) => Ok(CustomResult::new(json!({ "skill": entry }))),
                    None => Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "unknown skill".to_string(),
                        None,
                    )),
                }
            }
            other => Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                other.to_string(),
                None,
            )),
        }
    }
}

async fn peer_for(pool: &super::UpstreamPool, name: &str) -> rmcp::service::Peer<rmcp::RoleClient> {
    pool.connections
        .read()
        .await
        .get(name)
        .expect("connection registered")
        .peer
        .clone()
}

#[tokio::test]
async fn capability_detection_reads_the_declared_extension() {
    let declaring = SkillsServer::new(vec![json!({ "skills": [] })]);
    let pool = catalog_pool_with_server("declares", declaring).await;
    assert!(super::skills_list::peer_declares_skills(
        &peer_for(&pool, "declares").await
    ));

    let silent = SkillsServer::new(vec![json!({ "skills": [] })]).without_extension();
    let pool = catalog_pool_with_server("silent", silent).await;
    assert!(!super::skills_list::peer_declares_skills(
        &peer_for(&pool, "silent").await
    ));
}

#[tokio::test]
async fn walks_every_page_and_stops_when_the_cursor_clears() {
    let server = SkillsServer::new(vec![
        json!({ "skills": [entry("up", "alpha")], "nextCursor": "p2", "ttlMs": 60000, "cacheScope": "private" }),
        json!({ "skills": [entry("up", "beta")], "ttlMs": 30000, "cacheScope": "private" }),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;

    let skills = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect("walk succeeds");

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(skills.skills.len(), 2);
    assert!(!skills.truncated);
    // The snapshot is only as fresh as its stalest page.
    assert_eq!(skills.ttl_ms, Some(30_000));
    assert_eq!(skills.cache_scope.as_deref(), Some("private"));
}

#[tokio::test]
async fn an_empty_listing_is_not_an_error() {
    // A server whose catalog is generated or unenumerable returns nothing, and
    // that must not be read as failure — or as proof it has no skills.
    let server = SkillsServer::new(vec![json!({ "skills": [] })]);
    let pool = catalog_pool_with_server("empty", server).await;
    let skills = pool
        .fetch_upstream_skills("empty", &peer_for(&pool, "empty").await)
        .await
        .expect("an empty listing is a success");
    assert!(skills.skills.is_empty());
    assert!(!skills.truncated);
}

#[tokio::test]
async fn a_repeated_cursor_stops_the_walk() {
    // Same cursor forever: without the guard this would spin to the page cap.
    let server = SkillsServer::new(vec![
        json!({ "skills": [entry("up", "alpha")], "nextCursor": "same" }),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;

    let skills = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect("walk terminates");

    assert!(skills.truncated);
    // Two calls: the first mints the cursor, the second sees it repeat.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_never_repeating_cursor_is_bounded_by_the_page_cap() {
    // The subtler adversary: every page is distinct, so the repeat check never
    // fires and only the page cap terminates the walk.
    let pages: Vec<Value> = (0..limits::MAX_LIST_PAGES + 10)
        .map(|i| json!({ "skills": [], "nextCursor": format!("page-{i}") }))
        .collect();
    let server = SkillsServer::new(pages);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;

    let skills = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect("walk terminates");

    assert!(skills.truncated);
    assert_eq!(calls.load(Ordering::SeqCst), limits::MAX_LIST_PAGES);
}

#[tokio::test]
async fn the_skill_cap_stops_the_walk_before_the_next_request() {
    // Budgets must engage incrementally: one oversized page fills the cap, and
    // no further page is requested.
    let big: Vec<Value> = (0..limits::MAX_SKILLS_PER_UPSTREAM + 5)
        .map(|i| entry("up", &format!("skill-{i}")))
        .collect();
    let server = SkillsServer::new(vec![json!({ "skills": big, "nextCursor": "more" })]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;

    let skills = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect("walk terminates");

    assert!(skills.truncated);
    assert_eq!(skills.skills.len(), limits::MAX_SKILLS_PER_UPSTREAM);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no page fetched past the cap"
    );
}

#[tokio::test]
async fn one_malformed_skill_does_not_sink_the_upstream() {
    let good = entry("up", "alpha");
    // Manifest omits its own SKILL.md, which the SEP requires it to list.
    let bad = json!({
        "uri": "skill://up/broken/SKILL.md",
        "frontmatter": { "name": "broken", "description": "d" },
        "resources": [
            { "uri": "skill://up/broken/other.md", "digest": ResourceDigest::of_bytes(b"x").to_wire() }
        ]
    });
    // Generated skills publish no manifest and cannot be content-bound.
    let unverifiable = json!({
        "uri": "skill://up/generated/SKILL.md",
        "frontmatter": { "name": "generated", "description": "d" }
    });
    let server = SkillsServer::new(vec![json!({ "skills": [good, bad, unverifiable] })]);
    let pool = catalog_pool_with_server("up", server).await;

    let skills = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect("the upstream survives its own bad skills");

    assert_eq!(skills.skills.len(), 1);
    assert_eq!(skills.skills[0].name, "alpha");
    assert_eq!(skills.excluded_count(), 2);
    let reasons: Vec<SkillRejection> = skills.excluded.iter().map(|(r, _)| *r).collect();
    assert!(reasons.contains(&SkillRejection::ManifestMissingSkillMd));
    assert!(reasons.contains(&SkillRejection::MissingManifest));
}

#[tokio::test]
async fn a_cache_scope_change_mid_pagination_is_rejected() {
    // A server must apply one cacheScope to every page of a list; pages that
    // disagree do not describe a single listing.
    let server = SkillsServer::new(vec![
        json!({ "skills": [], "nextCursor": "p2", "cacheScope": "private" }),
        json!({ "skills": [], "cacheScope": "public" }),
    ]);
    let pool = catalog_pool_with_server("up", server).await;

    let error = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect_err("inconsistent cacheScope is rejected");
    assert!(error.contains("cacheScope"));
}

#[tokio::test]
async fn a_malformed_page_fails_rather_than_returning_a_partial_snapshot() {
    // The important half: an error, not the one good page. Caching a partial
    // walk as complete would make a later rediscover return the same gap.
    let server = SkillsServer::new(vec![
        json!({ "skills": [entry("up", "alpha")], "nextCursor": "p2" }),
        json!({ "skills": "not an array" }),
    ]);
    let pool = catalog_pool_with_server("up", server).await;

    let error = pool
        .fetch_upstream_skills("up", &peer_for(&pool, "up").await)
        .await
        .expect_err("a malformed page fails the walk");
    assert!(error.contains("malformed"));
}

#[tokio::test]
async fn skills_get_resolves_a_skill_that_never_appeared_in_a_listing() {
    // The unlisted-skill path: the listing is empty, but the URI still loads.
    let server = SkillsServer::new(vec![json!({ "skills": [] })]).with_get(entry("up", "unlisted"));
    let pool = catalog_pool_with_server("up", server).await;
    let peer = peer_for(&pool, "up").await;

    let listed = pool
        .fetch_upstream_skills("up", &peer)
        .await
        .expect("empty listing");
    assert!(listed.skills.is_empty());

    let fetched = pool
        .fetch_upstream_skill("up", &peer, "skill://up/unlisted/SKILL.md", None)
        .await
        .expect("skills/get succeeds")
        .expect("an unlisted skill still resolves");
    assert_eq!(fetched.name, "unlisted");
}

#[tokio::test]
async fn skills_get_treats_invalid_params_as_not_a_skill() {
    // -32602 is the one answer meaning "not a skill I serve"; it must not be
    // conflated with a transport failure, which would open the circuit for an
    // upstream that answered correctly.
    let server = SkillsServer::new(vec![json!({ "skills": [] })]);
    let pool = catalog_pool_with_server("up", server).await;

    let answer = pool
        .fetch_upstream_skill(
            "up",
            &peer_for(&pool, "up").await,
            "skill://up/nope/SKILL.md",
            None,
        )
        .await
        .expect("a -32602 is a successful negative answer");
    assert!(answer.is_none());
}
