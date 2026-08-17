//! Integration-style proof of the `GatewayManager` cold-reconstruction
//! boundary. Durable journal rows cross the boundary only after an explicit
//! flush; manager-owned history, promotion sources, buffers, and render caches
//! deliberately do not.

#![allow(clippy::disallowed_methods)] // fixture constructs rmcp Tool values

use std::sync::Arc;

use labby_codemode::{
    CodeModeExecutedCall, CodeModeHistoryEntry, CodeModeHistoryKind, CodeModeHost,
    CodeModeSourceLookup, CodeModeSurface, ExecCtx,
};
use rmcp::model::Tool;

use super::code_mode_host::JournalOwner;
use super::search::catalog_from_tools;
use crate::codemode_journal::StepJournalStore;
use crate::gateway::manager::GatewayManager;
use crate::gateway::runtime::GatewayRuntimeHandle;
use crate::upstream::types::UpstreamTool;

fn tool(name: &str, read_only: bool) -> UpstreamTool {
    let mut tool = Tool::new(
        name.to_string(),
        format!("{name} description"),
        Arc::new(serde_json::Map::new()),
    );
    tool.annotations = Some(
        rmcp::model::ToolAnnotations::new()
            .read_only(read_only)
            .destructive(!read_only),
    );
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::from("fixture"),
        destructive: !read_only,
    }
}

fn history_entry(execution_id: &str) -> CodeModeHistoryEntry {
    CodeModeHistoryEntry {
        execution_id: Some(execution_id.to_string()),
        seq: 1,
        route_scope: "route-a".to_string(),
        kind: CodeModeHistoryKind::Execute,
        ok: true,
        elapsed_ms: 1,
        input_tokens: None,
        output_tokens: None,
        error_kind: None,
        calls: Vec::<CodeModeExecutedCall>::new(),
        match_count: None,
    }
}

fn source(execution_id: &str) -> super::CodeModeExecutionSource {
    super::CodeModeExecutionSource {
        execution_id: execution_id.to_string(),
        created_at_ms: 1,
        actor_key: Some("actor-a".to_string()),
        is_admin: true,
        route_scope: "route-a".to_string(),
        surface: CodeModeSurface::Api,
        capability_filter_fingerprint: "cap-a".to_string(),
        code: "return 1".to_string(),
    }
}

fn source_lookup() -> CodeModeSourceLookup {
    CodeModeSourceLookup {
        actor_key: Some("actor-a".to_string()),
        is_admin: true,
        route_scope: "route-a".to_string(),
        capability_filter_fingerprint: "cap-a".to_string(),
    }
}

async fn manager(root: &std::path::Path) -> GatewayManager {
    let store = StepJournalStore::open(root.join("journal.db"))
        .await
        .expect("open journal");
    GatewayManager::new(root.join("config.toml"), GatewayRuntimeHandle::default())
        .with_step_journal(Arc::new(store))
}

#[tokio::test]
async fn cold_manager_reconstruction_preserves_only_explicitly_flushed_state() {
    let started = std::time::Instant::now();
    let root = tempfile::tempdir().expect("temporary persistent root");

    let manager_a = manager(root.path()).await;
    manager_a
        .record_step(
            ExecCtx {
                seq: 9,
                execution_id: Some(Arc::from("durable")),
                step_ordinal: Some(2),
            },
            &"n".repeat(100_000),
            &serde_json::json!({
                "authorization": "Bearer sk-abcdefghij0123456789extra"
            }),
        )
        .await
        .expect("buffer durable row");
    manager_a
        .flush_step_journal(
            "durable",
            &JournalOwner {
                actor_key: Some("actor-a".to_string()),
                route_scope: "route-a".to_string(),
                capability_filter_fingerprint: Some("cap-a".to_string()),
            },
        )
        .await;
    manager_a
        .record_step(
            ExecCtx {
                seq: 10,
                execution_id: Some(Arc::from("unflushed")),
                step_ordinal: Some(0),
            },
            "not-durable",
            &serde_json::json!(true),
        )
        .await
        .expect("buffer unflushed row");
    manager_a
        .record_code_mode_history(history_entry("durable"))
        .await;
    manager_a.record_code_mode_source(source("durable")).await;

    let render_a = catalog_from_tools(&manager_a, vec![tool("before_restart", true)], false)
        .await
        .expect("render manager A catalog");
    assert_eq!(render_a.entries[0].name, "before_restart");
    assert_eq!(manager_a.code_mode_history_snapshot().await.len(), 1);
    assert!(
        manager_a
            .resolve_code_mode_source("durable", &source_lookup())
            .await
            .is_ok()
    );

    // No clone, runner lease, store handle, or background persistence task is
    // retained across this point. Manager B is constructed independently from
    // the same persistent root.
    drop(render_a);
    drop(manager_a);

    let manager_b = manager(root.path()).await;
    let durable = manager_b
        .step_journal()
        .expect("journal configured")
        .load("durable")
        .await
        .expect("load durable row");
    assert_eq!(durable.len(), 1);
    assert_eq!(durable[0].step_ordinal, 2);
    assert_eq!(durable[0].seq_base, 9);
    assert_eq!(durable[0].actor_key.as_deref(), Some("actor-a"));
    assert_eq!(durable[0].route_scope, "route-a");
    assert_eq!(
        durable[0].capability_filter_fingerprint.as_deref(),
        Some("cap-a")
    );
    assert!(durable[0].name.len() <= 4_096);
    assert!(!durable[0].value.contains("sk-abcdefghij0123456789extra"));
    assert!(
        manager_b
            .step_journal()
            .expect("journal configured")
            .load("unflushed")
            .await
            .expect("load unflushed execution")
            .is_empty()
    );

    assert!(manager_b.code_mode_history_snapshot().await.is_empty());
    let source_error = manager_b
        .resolve_code_mode_source("durable", &source_lookup())
        .await
        .expect_err("promotion source is deliberately ephemeral");
    assert_eq!(source_error.kind(), "unknown_execution");

    // A changed live descriptor is rendered from Manager B's current input;
    // Manager A's descriptor and normalized safety facts are not reused.
    let render_b = catalog_from_tools(&manager_b, vec![tool("after_restart", false)], false)
        .await
        .expect("render manager B catalog");
    assert_eq!(render_b.entries.len(), 1);
    assert_eq!(render_b.entries[0].name, "after_restart");
    assert!(!render_b.catalog_json.contains("before_restart"));

    // The separately constructed manager is immediately usable for a safe,
    // non-executing Code Mode discovery operation. Runner/QuickJS process
    // replacement remains exhaustively covered by labby-codemode's pool tests;
    // this test owns only the manager-level reconstruction boundary.
    let discovery = labby_codemode::search_visible_tools(
        render_b.entries.as_ref(),
        &labby_codemode::ToolScope::default(),
        "after restart",
        5,
    )
    .expect("safe discovery after reconstruction");
    assert_eq!(discovery.results.len(), 1);

    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "focused reconstruction test exceeded its 5s wall-clock budget"
    );
}
