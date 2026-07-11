# Shared MCP Peer Catalog Notification Fanout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract one shared MCP peer catalog-change fanout helper so direct MCP server notifications and gateway peer notifications use identical timeout, liveness, and pruning semantics.

**Architecture:** Add a small `crates/labby/src/mcp/catalog_notifications.rs` module that owns the `Peer<RoleServer>` fanout mechanics and a protocol-local `CatalogNotificationChanges` value. Keep direct `CatalogChangeSet` and gateway `GatewayCatalogDiff` conversions at their call sites so gateway dispatch types do not leak into direct server notification code. Preserve the existing snapshot-plus-prune behavior, including peers that connect after the snapshot.

**Tech Stack:** Rust 2024, Tokio async runtime, rmcp `Peer<RoleServer>`, futures `join_all`, existing Labby tracing and config timeout helpers.

## Global Constraints

- Worktree: `/home/jmagar/.codex/worktrees/2fee521f-a65f-4819-9926-e457fa936a6f/lab/.worktrees/lab-p8yxv-2-peer-fanout`.
- Branch: `codex/lab-p8yxv-2-peer-fanout`, based on `origin/main=66247a2b53f739bdf26ac440d37c15c1b09f3ac9`.
- Do not edit the parent checkout or revert parallel agent work for `lab-p8yxv.1`.
- Follow `crates/labby/src/mcp/CLAUDE.md` when working under `crates/labby/src/mcp/`.
- Rust module style is sibling files only; do not create `mod.rs`.
- No new dependencies.
- Preserve `resolved_catalog_notification_timeout()` as the single timeout source.
- Preserve tools/resources/prompts list-changed coverage.
- Preserve stale-peer pruning and peers-added-after-snapshot behavior.

---

## File Structure

- Create `crates/labby/src/mcp/catalog_notifications.rs`: shared change-set struct, fanout helper, and focused tests for change conversion/no-op behavior.
- Modify `crates/labby/src/mcp.rs`: expose the new module as `pub(crate) mod catalog_notifications;`.
- Modify `crates/labby/src/mcp/server.rs`: remove local `join_all` import and delegate `LabMcpServer::notify_catalog_changes` to the shared helper.
- Modify `crates/labby/src/mcp/peers.rs`: remove duplicated fanout logic and delegate `PeerNotifier::notify_catalog_changes` to the shared helper after converting `GatewayCatalogDiff`.

### Task 1: Add Shared MCP Catalog Notification Helper

**Files:**
- Create: `crates/labby/src/mcp/catalog_notifications.rs`
- Modify: `crates/labby/src/mcp.rs`

**Interfaces:**
- Consumes: `Arc<RwLock<Vec<Peer<RoleServer>>>>`, `crate::config::resolved_catalog_notification_timeout()`, and rmcp peer notification methods.
- Produces:
  - `pub(crate) struct CatalogNotificationChanges { pub(crate) tools_changed: bool, pub(crate) resources_changed: bool, pub(crate) prompts_changed: bool }`
  - `impl CatalogNotificationChanges { pub(crate) const fn new(...) -> Self; pub(crate) const fn any(self) -> bool; }`
  - `pub(crate) async fn notify_catalog_peers(peers: &Arc<RwLock<Vec<Peer<RoleServer>>>>, changes: CatalogNotificationChanges, log_message: &'static str)`

- [ ] **Step 1: Read MCP-local rules**

Run: `sed -n '1,180p' crates/labby/src/mcp/CLAUDE.md`
Expected: Read the MCP dispatch and module rules before editing.

- [ ] **Step 2: Add the module export**

Edit `crates/labby/src/mcp.rs` and add this line near `pub mod catalog;`:

```rust
pub(crate) mod catalog_notifications;
```

- [ ] **Step 3: Create the shared helper**

Create `crates/labby/src/mcp/catalog_notifications.rs` with:

```rust
use std::sync::Arc;

use futures::future::join_all;
use rmcp::RoleServer;
use rmcp::service::Peer;
use tokio::sync::RwLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogNotificationChanges {
    pub(crate) tools_changed: bool,
    pub(crate) resources_changed: bool,
    pub(crate) prompts_changed: bool,
}

impl CatalogNotificationChanges {
    pub(crate) const fn new(
        tools_changed: bool,
        resources_changed: bool,
        prompts_changed: bool,
    ) -> Self {
        Self {
            tools_changed,
            resources_changed,
            prompts_changed,
        }
    }

    pub(crate) const fn any(self) -> bool {
        self.tools_changed || self.resources_changed || self.prompts_changed
    }
}

pub(crate) async fn notify_catalog_peers(
    peers: &Arc<RwLock<Vec<Peer<RoleServer>>>>,
    changes: CatalogNotificationChanges,
    log_message: &'static str,
) {
    if !changes.any() {
        return;
    }

    let peer_snapshot = peers.read().await.clone();
    let peer_count = peer_snapshot.len();
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "catalog.notify",
        subsystem = "mcp_server",
        phase = "catalog.notify",
        peer_count,
        tools_changed = changes.tools_changed,
        resources_changed = changes.resources_changed,
        prompts_changed = changes.prompts_changed,
        "{log_message}"
    );

    let notification_timeout = crate::config::resolved_catalog_notification_timeout();
    let notify_futures = peer_snapshot.iter().enumerate().map(|(peer_index, peer)| {
        let peer = peer.clone();
        async move {
            let result = tokio::time::timeout(notification_timeout, async {
                if changes.tools_changed && peer.notify_tool_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "tools",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                if changes.resources_changed && peer.notify_resource_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "resources",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                if changes.prompts_changed && peer.notify_prompt_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "prompts",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                true
            })
            .await;

            match result {
                Ok(alive) => alive,
                Err(_elapsed) => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        timeout_ms = notification_timeout.as_millis(),
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "peer notification timed out; pruning stale session"
                    );
                    false
                }
            }
        }
    });

    let results = join_all(notify_futures).await;
    let alive: Vec<_> = peer_snapshot
        .into_iter()
        .zip(results)
        .filter_map(|(peer, ok)| ok.then_some(peer))
        .collect();

    let alive_count = alive.len();
    let mut guard = peers.write().await;
    let added_since_snapshot = if guard.len() > peer_count {
        guard.split_off(peer_count)
    } else {
        Vec::new()
    };
    *guard = alive;
    guard.extend(added_since_snapshot);
    let pruned = peer_count.saturating_sub(alive_count);
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "peer.gc",
        pruned_count = pruned,
        active_count = guard.len(),
        "MCP peer catalog-change notification complete"
    );
}

#[cfg(test)]
mod tests {
    use super::CatalogNotificationChanges;

    #[test]
    fn catalog_notification_changes_reports_any_changed_kind() {
        assert!(!CatalogNotificationChanges::new(false, false, false).any());
        assert!(CatalogNotificationChanges::new(true, false, false).any());
        assert!(CatalogNotificationChanges::new(false, true, false).any());
        assert!(CatalogNotificationChanges::new(false, false, true).any());
    }
}
```

- [ ] **Step 4: Verify the focused helper test passes**

Run: `cargo test -p labby mcp::catalog_notifications::tests::catalog_notification_changes_reports_any_changed_kind --all-features -- --nocapture`

Expected: The test passes.

- [ ] **Step 5: Commit task 1**

```bash
git add crates/labby/src/mcp.rs crates/labby/src/mcp/catalog_notifications.rs
git commit -m "refactor(mcp): add shared catalog notification helper"
```

### Task 2: Route Direct Server Notifications Through the Helper

**Files:**
- Modify: `crates/labby/src/mcp/server.rs`
- Test: existing MCP call-tool paths that trigger `LabMcpServer::notify_catalog_changes`

**Interfaces:**
- Consumes: `crate::mcp::catalog::CatalogChangeSet`
- Produces: direct server `notify_catalog_changes` delegating to `notify_catalog_peers`

- [ ] **Step 1: Remove duplicated imports**

In `crates/labby/src/mcp/server.rs`, remove:

```rust
use futures::future::join_all;
```

- [ ] **Step 2: Replace direct fanout body**

Replace the entire `impl LabMcpServer { pub(crate) async fn notify_catalog_changes... }` body with:

```rust
impl LabMcpServer {
    pub(crate) async fn notify_catalog_changes(&self, changes: CatalogChangeSet) {
        crate::mcp::catalog_notifications::notify_catalog_peers(
            &self.peers,
            crate::mcp::catalog_notifications::CatalogNotificationChanges::new(
                changes.tools_changed,
                changes.resources_changed,
                changes.prompts_changed,
            ),
            "notifying MCP peers about catalog change",
        )
        .await;
    }
}
```

- [ ] **Step 3: Verify direct path still compiles**

Run: `cargo test -p labby server_capabilities_advertise_list_changed_support --all-features -- --nocapture`

Expected: The test passes and `server.rs` compiles without the old `join_all` import.

- [ ] **Step 4: Commit task 2**

```bash
git add crates/labby/src/mcp/server.rs
git commit -m "refactor(mcp): reuse catalog notification fanout in server"
```

### Task 3: Route Gateway PeerNotifier Through the Helper

**Files:**
- Modify: `crates/labby/src/mcp/peers.rs`
- Test: existing gateway-backed MCP tests

**Interfaces:**
- Consumes: `crate::dispatch::gateway::types::GatewayCatalogDiff`
- Produces: gateway `PeerNotifier::notify_catalog_changes` delegating to the same helper

- [ ] **Step 1: Remove duplicated imports**

In `crates/labby/src/mcp/peers.rs`, remove:

```rust
use futures::future::join_all;
```

- [ ] **Step 2: Replace gateway fanout body**

Replace the body of `PeerNotifier::notify_catalog_changes` with:

```rust
    #[cfg(feature = "gateway")]
    async fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        crate::mcp::catalog_notifications::notify_catalog_peers(
            &self.peers,
            crate::mcp::catalog_notifications::CatalogNotificationChanges::new(
                diff.tools_changed,
                diff.resources_changed,
                diff.prompts_changed,
            ),
            "broadcasting catalog change to connected peers",
        )
        .await;
    }
```

- [ ] **Step 3: Verify gateway PeerNotifier callers still compile**

Run: `cargo test -p labby server_reads_current_pool_from_gateway_manager --all-features -- --nocapture`

Expected: The test passes and `peers.rs` compiles without local fanout logic.

- [ ] **Step 4: Commit task 3**

```bash
git add crates/labby/src/mcp/peers.rs
git commit -m "refactor(mcp): reuse catalog notification fanout for gateway peers"
```

### Task 4: Final Verification and Review Readiness

**Files:**
- No new implementation files beyond Tasks 1-3.
- Update this plan checklist only if the implementation agent tracks completed steps in-place.

**Interfaces:**
- Consumes: all changes from Tasks 1-3.
- Produces: a green branch ready for PR and review.

- [ ] **Step 1: Format**

Run: `cargo fmt --all --check`

Expected: Pass. If it fails, run `cargo fmt --all`, inspect the diff, and rerun `cargo fmt --all --check`.

- [ ] **Step 2: Run focused MCP notification and gateway tests**

Run:

```bash
cargo test -p labby mcp::catalog_notifications --all-features -- --nocapture
cargo test -p labby server_capabilities_advertise_list_changed_support --all-features -- --nocapture
cargo test -p labby server_reads_current_pool_from_gateway_manager --all-features -- --nocapture
```

Expected: All pass.

- [ ] **Step 3: Run the all-features compile gate**

Run: `cargo build -p labby --all-features`

Expected: Pass with no new warnings from the changed files.

- [ ] **Step 4: Inspect final diff**

Run: `git diff --stat origin/main...HEAD && git diff origin/main...HEAD -- crates/labby/src/mcp.rs crates/labby/src/mcp/catalog_notifications.rs crates/labby/src/mcp/server.rs crates/labby/src/mcp/peers.rs`

Expected: Diff is limited to the shared helper, module registration, and call-site rewiring.

- [ ] **Step 5: Commit final verification notes only if needed**

If Task 4 changed only formatting from prior commits:

```bash
git add crates/labby/src/mcp.rs crates/labby/src/mcp/catalog_notifications.rs crates/labby/src/mcp/server.rs crates/labby/src/mcp/peers.rs
git commit -m "style(mcp): format shared catalog notification fanout"
```

If no files changed, do not create an empty commit.

## Self-Review

Spec coverage:
- Shared helper covers tools/resources/prompts via `CatalogNotificationChanges`.
- Direct server and gateway `PeerNotifier` both call `notify_catalog_peers`.
- Timeout/pruning semantics are preserved by copying the snapshot, timeout, failed notification, and peers-added-after-snapshot logic into one helper.
- Existing catalog notification tests are covered by focused server/gateway compile tests plus the new helper unit test.

Placeholder scan:
- No TBD, TODO, or unfilled implementation references remain.

Type consistency:
- `CatalogNotificationChanges::new(bool, bool, bool) -> Self` is used consistently by both call sites.
- `notify_catalog_peers(&Arc<RwLock<Vec<Peer<RoleServer>>>>, CatalogNotificationChanges, &'static str)` matches both `LabMcpServer.peers` and `PeerNotifier.peers`.
