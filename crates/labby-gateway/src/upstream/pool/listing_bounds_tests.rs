//! Focused regressions for bounded upstream catalog listing passes: the
//! `MAX_LIST_PAGES` pagination cap, repeated-cursor loop breaking, truncation
//! visibility in the status channel, and the per-upstream listing wall-clock
//! bound. Kept here rather than in the capability listing modules so those
//! files stay near the 500-LOC target.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rmcp::model::{
    ErrorData, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, Prompt, Resource, ResourceTemplate,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::paginate::MAX_LIST_PAGES;
use super::testsupport::*;
use super::{SubjectScopedConnection, UpstreamPool};

fn oauth_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
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

/// A malicious/buggy upstream whose `nextCursor` always points back at the
/// same cursor value (`"loop"`), for both resources and templates.
#[derive(Clone, Default)]
struct LoopingCursorResourceServer {
    resource_calls: Arc<AtomicUsize>,
    template_calls: Arc<AtomicUsize>,
}

impl ServerHandler for LoopingCursorResourceServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let page = self.resource_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut result = ListResourcesResult::with_all_items(vec![Resource::new(
            format!("file:///page-{page}"),
            format!("page-{page}"),
        )]);
        result.next_cursor = Some("loop".to_string());
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let page = self.template_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut result = ListResourceTemplatesResult::with_all_items(vec![ResourceTemplate::new(
            format!("file:///page-{page}/{{path}}"),
            format!("page-{page}"),
        )]);
        result.next_cursor = Some("loop".to_string());
        Ok(result)
    }
}

/// An upstream that mints a fresh `nextCursor` on every page, forever.
#[derive(Clone, Default)]
struct EndlessCursorResourceServer {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for EndlessCursorResourceServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let page = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut result = ListResourcesResult::with_all_items(vec![Resource::new(
            format!("file:///page-{page}"),
            format!("page-{page}"),
        )]);
        result.next_cursor = Some(format!("page-{page}"));
        Ok(result)
    }
}

/// A malicious/buggy upstream whose prompt `nextCursor` always points back at
/// the same cursor value.
#[derive(Clone, Default)]
struct LoopingCursorPromptServer {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for LoopingCursorPromptServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let page = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut result = ListPromptsResult::with_all_items(vec![Prompt::new(
            format!("page-{page}"),
            Some("looping page"),
            None,
        )]);
        result.next_cursor = Some("loop".to_string());
        Ok(result)
    }
}

/// A malicious/buggy upstream whose tool `nextCursor` always points back at
/// the same cursor value.
#[derive(Clone, Default)]
struct LoopingCursorToolServer {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for LoopingCursorToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let page = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut result = ListToolsResult::with_all_items(vec![Tool::new(
            format!("tool-page-{page}"),
            "looping page",
            Arc::new(serde_json::Map::new()),
        )]);
        result.next_cursor = Some("loop".to_string());
        Ok(result)
    }
}

/// A server whose listings stall long past the listing wall-clock budget.
struct SlowListServer;

impl ServerHandler for SlowListServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "slow",
            Some("slow prompt"),
            None,
        )]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("file:///{path}", "slow"),
        ]))
    }
}

#[tokio::test]
async fn resource_catalog_breaks_a_looping_cursor() {
    let server = LoopingCursorResourceServer::default();
    let calls = Arc::clone(&server.resource_calls);
    let pool = catalog_pool_with_server("looping", server).await;

    let resources = pool.list_upstream_resources().await;

    // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
    assert_eq!(resources.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // The partial data is cached and the upstream stays routable, but the
    // truncation is surfaced through the status channel rather than the
    // pass reading as a clean success.
    let catalog = pool.catalog.read().await;
    let entry = catalog.get("looping").expect("looping catalog entry");
    assert_eq!(entry.resource_count, 2);
    assert!(entry.resource_health.is_routable());
    assert_eq!(
        entry.resource_last_error.as_deref(),
        Some("resources/list truncated (cursor_loop) after 2 pages — upstream catalog is partial")
    );
}

#[tokio::test]
async fn resource_template_catalog_breaks_a_looping_cursor() {
    let server = LoopingCursorResourceServer::default();
    let calls = Arc::clone(&server.template_calls);
    let pool = catalog_pool_with_server("looping", server).await;

    let templates = pool.list_upstream_resource_templates_allowed(None).await;

    assert_eq!(templates.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resource_catalog_stops_at_the_page_cap_when_cursors_never_repeat() {
    let server = EndlessCursorResourceServer::default();
    let calls = Arc::clone(&server.calls);
    let pool = catalog_pool_with_server("endless", server).await;

    let resources = pool.list_upstream_resources().await;

    assert_eq!(resources.len(), MAX_LIST_PAGES);
    assert_eq!(calls.load(Ordering::SeqCst), MAX_LIST_PAGES);
}

/// Pins the OAuth subject-scoped tier to the bounded helper: reverting
/// `subject_scoped_resources` to rmcp's `list_all_resources` would
/// reintroduce unbounded pagination that no catalog-tier test can catch.
#[tokio::test]
async fn subject_scoped_resources_break_a_looping_cursor() {
    let server = LoopingCursorResourceServer::default();
    let calls = Arc::clone(&server.resource_calls);
    let pool = catalog_pool_with_server("looping", server).await;
    seed_subject_connection(&pool, "looping", "alice").await;
    let mut config = oauth_config("looping");
    config.proxy_resources = true;

    let resources = pool.subject_scoped_resources(&[config], "alice").await;

    // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
    assert_eq!(resources.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[derive(Clone, Default)]
struct PaginatedPromptServer {
    calls: Arc<AtomicUsize>,
}

impl ServerHandler for PaginatedPromptServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let cursor = request.and_then(|request| request.cursor);
        let mut result = match cursor.as_deref() {
            None => ListPromptsResult::with_all_items(vec![Prompt::new(
                "first",
                Some("first page"),
                None,
            )]),
            Some("page-2") => ListPromptsResult::with_all_items(vec![Prompt::new(
                "second",
                Some("second page"),
                None,
            )]),
            Some(other) => {
                return Err(ErrorData::invalid_params(
                    format!("unexpected cursor: {other}"),
                    None,
                ));
            }
        };
        if cursor.is_none() {
            result.next_cursor = Some("page-2".to_string());
        }
        Ok(result)
    }
}

#[tokio::test]
async fn prompt_catalog_traverses_all_upstream_pages() {
    let server = PaginatedPromptServer::default();
    let calls = Arc::clone(&server.calls);
    let pool = catalog_pool_with_server("paged", server).await;

    let prompts = pool.list_upstream_prompts(&[]).await;
    let names = prompts
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["paged/first", "paged/second"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        pool.catalog
            .read()
            .await
            .get("paged")
            .expect("paged catalog entry")
            .prompt_count,
        2
    );
}

#[tokio::test]
async fn prompt_catalog_breaks_a_looping_cursor() {
    let server = LoopingCursorPromptServer::default();
    let calls = Arc::clone(&server.calls);
    let pool = catalog_pool_with_server("looping", server).await;

    let prompts = pool.list_upstream_prompts(&[]).await;
    let names = prompts
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect::<Vec<_>>();

    // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
    assert_eq!(names, vec!["looping/page-1", "looping/page-2"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // The partial data is cached and the upstream stays routable, but the
    // truncation is surfaced through the status channel rather than the
    // pass reading as a clean success.
    let catalog = pool.catalog.read().await;
    let entry = catalog.get("looping").expect("looping catalog entry");
    assert_eq!(entry.prompt_count, 2);
    assert!(entry.prompt_health.is_routable());
    assert_eq!(
        entry.prompt_last_error.as_deref(),
        Some("prompts/list truncated (cursor_loop) after 2 pages — upstream catalog is partial")
    );
}

/// Pins `subject_scoped_prompt_owner` to the bounded helper: ownership
/// resolution lists the whole prompt catalog per upstream, so a looping
/// cursor here would otherwise spin for the full connection lifetime.
#[tokio::test]
async fn subject_scoped_prompt_owner_is_bounded_on_a_looping_cursor() {
    let server = LoopingCursorPromptServer::default();
    let calls = Arc::clone(&server.calls);
    let pool = catalog_pool_with_server("looping", server).await;
    seed_subject_connection(&pool, "looping", "alice").await;
    let config = oauth_config("looping");

    let owner = pool
        .subject_scoped_prompt_owner(std::slice::from_ref(&config), "alice", "looping/page-1")
        .await;

    assert_eq!(owner.as_deref(), Some("looping"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// Pins the notification-driven tool re-listing (a catalog publication path)
/// to the bounded helper, and truncation to the status channel.
#[tokio::test]
async fn tool_refresh_breaks_a_looping_cursor() {
    let server = LoopingCursorToolServer::default();
    let calls = Arc::clone(&server.calls);
    let pool = catalog_pool_with_server("looping", server).await;

    let refreshed = pool.refresh_tools_after_list_changed("looping").await;

    assert!(refreshed, "a truncated listing still refreshes the catalog");
    // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let catalog = pool.catalog.read().await;
    let entry = catalog.get("looping").expect("looping catalog entry");
    assert_eq!(entry.tools.len(), 2);
    assert!(entry.tool_health.is_routable());
    assert_eq!(
        entry.tool_last_error.as_deref(),
        Some("tools/list truncated (cursor_loop) after 2 pages — upstream catalog is partial")
    );
}

#[tokio::test]
async fn prompt_catalog_bounds_a_stalled_upstream() {
    let pool = catalog_pool_with_server("slow", SlowListServer).await;
    let mut pool = Arc::try_unwrap(pool)
        .ok()
        .expect("fixture pool has one owner");
    pool.request_timeout = Duration::from_millis(25);

    let started = Instant::now();
    let prompts = pool.list_upstream_prompts(&[]).await;

    assert!(
        prompts.is_empty(),
        "a timed-out upstream yields partial data"
    );
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "a stalled prompt listing exceeded the request budget: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn resource_template_catalog_bounds_a_stalled_upstream() {
    let pool = catalog_pool_with_server("slow", SlowListServer).await;
    let mut pool = Arc::try_unwrap(pool)
        .ok()
        .expect("fixture pool has one owner");
    pool.request_timeout = Duration::from_millis(25);

    let started = Instant::now();
    let templates = pool.list_upstream_resource_templates_allowed(None).await;

    assert!(
        templates.is_empty(),
        "a timed-out upstream yields partial data"
    );
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "a stalled template listing exceeded the request budget: {:?}",
        started.elapsed()
    );
}
