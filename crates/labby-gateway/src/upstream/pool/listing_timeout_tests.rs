//! Focused regressions for the per-upstream listing wall-clock bounds: a
//! stalled page RPC must degrade that upstream to partial data within the
//! `listing_catalog_timeout` budget instead of holding the merged listing
//! open. Split from `listing_bounds_tests.rs` to keep both under the 500-LOC
//! target.
//!
//! The elapsed-time assertions are deliberately loose (well above the mock's
//! 200ms stall, far below unbounded): tight bounds flaked under parallel test
//! load from OS descheduling. The emptiness assertions are what prove the
//! budget fired.

#![cfg(test)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::{
    ErrorData, ListPromptsResult, ListResourceTemplatesResult, PaginatedRequestParams, Prompt,
    ResourceTemplate, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};

use super::testsupport::*;

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
        started.elapsed() < Duration::from_secs(5),
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
        started.elapsed() < Duration::from_secs(5),
        "a stalled template listing exceeded the request budget: {:?}",
        started.elapsed()
    );
}
