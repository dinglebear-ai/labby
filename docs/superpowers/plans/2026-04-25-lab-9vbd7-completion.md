# lab-9vbd.7 extract_error_info Downcast Integrity Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Add focused tests proving `extract_error_info` preserves structured MCP error details through both the `DispatchError` downcast path and serialized JSON fallback path.

**Architecture:** The tests live next to `extract_error_info` in `crates/lab/src/mcp/server.rs` because the function is MCP-local and currently tested from that module. The downcast test calls the real always-on `lab_admin` dispatch path with an unknown action, converts its `ToolError` into `DispatchError`, wraps it in `anyhow::Error`, then calls `extract_error_info`. The fallback test builds a serialized JSON error string and verifies `extract_error_info` recovers the stable kind, message, and extras without a typed downcast.

**Tech Stack:** Rust 2024, `tokio::test`, `anyhow::Error`, `serde_json`, existing `ToolError`/`DispatchError` MCP error types.

---

### Task 1: Add focused extraction tests

**Files:**
- Modify: `crates/lab/src/mcp/server.rs`

- [x] **Step 1: Extend test imports**

Update the `#[cfg(test)] mod tests` imports to include `extract_error_info`, `DispatchError`, and `serde_json::json`.

- [x] **Step 2: Add downcast integrity test**

Add a test named so `cargo test -- extract_error` selects it:

```rust
#[tokio::test]
async fn extract_error_info_preserves_unknown_action_from_real_dispatch_downcast() {
    let err = crate::dispatch::lab_admin::dispatch(
        "definitely.unknown",
        json!({}),
    )
    .await
    .expect_err("unknown lab_admin action should fail");
    let dispatch_error = DispatchError::from(err);
    let anyhow_error = anyhow::Error::from(dispatch_error);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(message, "unknown action `lab_admin.definitely.unknown`");
    let extra = extra.expect("unknown_action should preserve valid action extras");
    assert_eq!(extra["valid"][0], "help");
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], Value::Null);
}
```

- [x] **Step 3: Add serialized JSON fallback test**

Add a second test named so `cargo test -- extract_error` selects it:

```rust
#[test]
fn extract_error_info_preserves_unknown_action_from_json_fallback() {
    let serialized = json!({
        "kind": "unknown_action",
        "message": "unknown action `movie.serch` for service `radarr`",
        "valid": ["movie.search", "movie.add"],
        "hint": "movie.search"
    })
    .to_string();
    let anyhow_error = anyhow::anyhow!(serialized);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(message, "unknown action `movie.serch` for service `radarr`");
    let extra = extra.expect("json fallback should preserve structured extras");
    assert_eq!(extra["valid"], json!(["movie.search", "movie.add"]));
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], json!("movie.search"));
}
```

- [x] **Step 4: Run required extraction verification**

Run: `cargo test -- extract_error`

Expected: tests matching `extract_error` pass, including both new tests.

- [x] **Step 5: Run relevant focused MCP tests**

Run: `cargo test -p lab mcp::server::tests`

Expected: MCP server unit tests pass.

### Task 2: Write completion session report

**Files:**
- Create: `docs/sessions/2026-04-25-lab-9vbd7-completion.md`

- [x] **Step 1: Gather required metadata**

Run the commands required by the user request and capture factual outputs in the report:

```bash
TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'
git remote get-url origin
git branch --show-current
git rev-parse --short HEAD
git log --oneline -5
git status --short
git log --oneline --name-only -10
pwd
git worktree list | grep $(pwd) | head -1
gh pr view --json number,title,url 2>/dev/null || echo "none"
```

- [x] **Step 2: Document the work**

Create the report with YAML metadata and the required sections: User Request, Session Overview, Sequence of Events, Key Findings, Technical Decisions, Files Modified, Commands Executed, Errors Encountered if any, Behavior Changes, Verification Evidence, Risks and Rollback, Decisions Not Taken if any, References if any, Open Questions if any, Next Steps.

- [x] **Step 3: Confirm bead closeability criteria**

Ensure the report records that the validation criteria are covered: real dispatch unknown action through `DispatchError`, JSON fallback, `kind == "unknown_action"`, and extras preserved.
