# MCP List Pagination Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make MCP `tools/list`, `resources/list`, and `prompts/list` collect only the requested page plus one lookahead item where practical, instead of building full local catalogs before slicing.

**Architecture:** Keep the existing offset cursor contract and `invalid_cursor` envelope. Add a small bounded page collector in `crates/labby/src/mcp/pagination.rs` that accepts already-filtered items in stable order, counts accepted items globally, stores only the requested page plus one sentinel, and exposes a `finished()` predicate so handlers can stop local source iteration early. Refactor handlers to use the collector for local/builtin sources first; upstream pool APIs still return vectors, so consume them through the same collector and stop further request-local work once the page and next-cursor sentinel are known.

**Tech Stack:** Rust 2024, `rmcp::model::PaginatedRequestParams`, Tokio async tests, existing `labby` MCP handler test harness.

## Global Constraints

- Work in `/home/jmagar/.codex/worktrees/2fee521f-a65f-4819-9926-e457fa936a6f/lab/.worktrees/lab-p8yxv-1-pagination` on branch `codex/lab-p8yxv-1-pagination`.
- Preserve integer offset cursors and `kind: "invalid_cursor"` error data.
- Preserve MCP page size `MCP_LIST_PAGE_SIZE = 100`.
- Preserve route-scope, Code Mode visibility, OAuth subject-scope, logging, and tool-name de-dup semantics.
- Do not touch sibling bead `lab-p8yxv.2` notification fanout code.
- Use modern Rust module style; do not add `mod.rs`.
- Verify with focused MCP pagination tests plus `cargo check -p labby --all-features`.

---

### Task 1: Add Bounded Page Collection Primitive

**Files:**
- Modify: `crates/labby/src/mcp/pagination.rs`

**Interfaces:**
- Consumes: `PaginatedRequestParams`, `MCP_LIST_PAGE_SIZE`, existing `invalid_cursor`.
- Produces: `PageCollector<T>`, `try_collect_page`, and tests used by handler refactors.

- [ ] **Step 1: Write failing pagination tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/labby/src/mcp/pagination.rs`:

```rust
#[test]
fn page_collector_stops_after_page_plus_lookahead() {
    let mut collector = PageCollector::new(None).expect("collector");
    let mut visited = 0;

    for item in 0..250 {
        visited += 1;
        collector.accept(item);
        if collector.finished() {
            break;
        }
    }

    let (page, next_cursor) = collector.finish();
    assert_eq!(visited, MCP_LIST_PAGE_SIZE + 1);
    assert_eq!(page, (0..MCP_LIST_PAGE_SIZE).collect::<Vec<_>>());
    assert_eq!(next_cursor.as_deref(), Some("100"));
}

#[test]
fn page_collector_counts_skipped_items_without_storing_them() {
    let request = PaginatedRequestParams::default().with_cursor(Some("200".to_string()));
    let mut collector = PageCollector::new(Some(request)).expect("collector");
    let mut visited = 0;

    for item in 0..250 {
        visited += 1;
        collector.accept(item);
        if collector.finished() {
            break;
        }
    }

    let (page, next_cursor) = collector.finish();
    assert_eq!(visited, 250);
    assert_eq!(page, (200..250).collect::<Vec<_>>());
    assert_eq!(next_cursor, None);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run: `cargo test -p labby mcp::pagination::tests::page_collector_ -- --nocapture`

Expected: FAIL because `PageCollector` does not exist.

- [ ] **Step 3: Implement `PageCollector`**

Add this implementation above `paginate_items` in `crates/labby/src/mcp/pagination.rs`:

```rust
pub(crate) struct PageCollector<T> {
    start: usize,
    seen: usize,
    page: Vec<T>,
    has_next: bool,
}

impl<T> PageCollector<T> {
    pub(crate) fn new(request: Option<PaginatedRequestParams>) -> Result<Self, ErrorData> {
        let start = match request.and_then(|request| request.cursor) {
            Some(cursor) => parse_cursor(&cursor)?,
            None => 0,
        };
        Ok(Self {
            start,
            seen: 0,
            page: Vec::with_capacity(MCP_LIST_PAGE_SIZE),
            has_next: false,
        })
    }

    pub(crate) fn accept(&mut self, item: T) {
        if self.finished() {
            return;
        }
        if self.seen < self.start {
            self.seen += 1;
            return;
        }
        if self.page.len() < MCP_LIST_PAGE_SIZE {
            self.page.push(item);
            self.seen += 1;
            return;
        }
        self.has_next = true;
        self.seen += 1;
    }

    pub(crate) fn finished(&self) -> bool {
        self.has_next
    }

    pub(crate) fn finish(self) -> (Vec<T>, Option<String>) {
        let next_cursor = self.has_next.then(|| (self.start + self.page.len()).to_string());
        (self.page, next_cursor)
    }
}

pub(crate) fn try_collect_page<T, I>(
    items: I,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData>
where
    I: IntoIterator<Item = T>,
{
    let mut collector = PageCollector::new(request)?;
    for item in items {
        collector.accept(item);
        if collector.finished() {
            break;
        }
    }
    Ok(collector.finish())
}
```

- [ ] **Step 4: Preserve existing `paginate_items` API**

Change `paginate_items` to delegate to the new helper:

```rust
pub(crate) fn paginate_items<T>(
    items: Vec<T>,
    request: Option<PaginatedRequestParams>,
) -> Result<(Vec<T>, Option<String>), ErrorData> {
    let start = match request.as_ref().and_then(|request| request.cursor.as_ref()) {
        Some(cursor) => parse_cursor(cursor)?,
        None => 0,
    };
    if start > items.len() {
        return Err(invalid_cursor("cursor is past the end of the result set"));
    }
    try_collect_page(items, request)
}
```

- [ ] **Step 5: Run pagination tests**

Run: `cargo test -p labby mcp::pagination::tests -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/mcp/pagination.rs
git commit -m "refactor(mcp): add bounded pagination collector"
```

### Task 2: Refactor Tools List To Use Bounded Collection

**Files:**
- Modify: `crates/labby/src/mcp/handlers_tools.rs`
- Modify: `crates/labby/src/mcp/handlers_tools/tests.rs`

**Interfaces:**
- Consumes: `PageCollector<Tool>` from Task 1.
- Produces: `list_tools_impl` that builds at most page plus lookahead for accepted tools while still running de-duplication over skipped accepted items.

- [ ] **Step 1: Write failing bounded local catalog test**

In `crates/labby/src/mcp/handlers_tools/tests.rs`, update `list_tools_paginates_large_builtin_catalog` to assert only the first page and lookahead are constructed. Add a static counter near the test helper area:

```rust
static TOOL_DESCRIPTION_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
```

Update the `large_test_registry` helper so each synthetic service description calls a helper:

```rust
fn counted_description(index: usize) -> String {
    TOOL_DESCRIPTION_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("Synthetic service {index:03}")
}
```

Before calling `list_tools_impl(None, ...)`, reset the counter:

```rust
TOOL_DESCRIPTION_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
```

After first page assertions, add:

```rust
assert_eq!(
    TOOL_DESCRIPTION_COUNT.load(std::sync::atomic::Ordering::SeqCst),
    crate::mcp::pagination::MCP_LIST_PAGE_SIZE + 1,
    "first-page collection should stop after page plus next-cursor lookahead"
);
```

- [ ] **Step 2: Run focused test and verify it fails**

Run: `cargo test -p labby list_tools_paginates_large_builtin_catalog -- --nocapture`

Expected: FAIL because the current handler constructs all 250 synthetic tool descriptions.

- [ ] **Step 3: Refactor handler imports**

In `crates/labby/src/mcp/handlers_tools.rs`, change the pagination import to:

```rust
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
```

- [ ] **Step 4: Initialize collector before catalog loops**

Replace `let mut tools = Vec::new();` with:

```rust
let mut tools = match PageCollector::new(request) {
    Ok(collector) => collector,
    Err(error) => {
        let elapsed_ms = start.elapsed().as_millis();
        let kind = pagination_error_kind(&error);
        tracing::warn!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            elapsed_ms,
            kind,
            "tool list failed"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Warning,
                kind,
            },
        )
        .await;
        return Err(error);
    }
};
```

- [ ] **Step 5: Replace accepted `tools.push(...)` calls**

For each accepted tool, call `tools.accept(...)` and then stop only when it is safe to stop the current logical source. For builtin services:

```rust
tools.accept(Tool::new(svc.name, svc.description, Arc::clone(&schema)));
builtin_tool_count += 1;
if tools.finished() {
    break;
}
```

For Code Mode and upstream branches, keep de-duplication before `accept`, then call:

```rust
tools.accept(ut.tool);
if hide_raw_tools {
    upstream_ui_tool_count += 1;
} else {
    upstream_tool_count += 1;
}
if tools.finished() {
    break;
}
```

In subject-scoped tool loops, keep `advertised_names.insert(...)` before `accept` so skipped accepted tools still reserve their names:

```rust
tools.accept(ut);
subject_scoped_tool_count += 1;
if tools.finished() {
    break;
}
```

- [ ] **Step 6: Finish collector instead of calling `paginate_items`**

Replace the old `let total_tool_count = tools.len();` and `paginate_items(...)` block with:

```rust
let total_tool_count = builtin_tool_count
    + gateway_tool_count
    + upstream_tool_count
    + upstream_ui_tool_count
    + subject_scoped_tool_count;
let (tools, next_cursor) = tools.finish();
```

- [ ] **Step 7: Run tools pagination tests**

Run: `cargo test -p labby list_tools_paginates_large_builtin_catalog -- --nocapture`

Expected: PASS.

Run: `cargo test -p labby list_tools_rejects_invalid_cursor -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/labby/src/mcp/handlers_tools.rs crates/labby/src/mcp/handlers_tools/tests.rs
git commit -m "perf(mcp): bound tools list page collection"
```

### Task 3: Refactor Resources And Prompts Lists

**Files:**
- Modify: `crates/labby/src/mcp/handlers_resources.rs`
- Modify: `crates/labby/src/mcp/handlers_prompts.rs`

**Interfaces:**
- Consumes: `PageCollector<Resource>` and `PageCollector<Prompt>`.
- Produces: bounded local resource/prompt collection with unchanged response types.

- [ ] **Step 1: Refactor imports**

In both files, replace `paginate_items` import with `PageCollector`:

```rust
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
```

- [ ] **Step 2: Initialize collectors with existing error logging**

In each handler, replace `let mut resources = vec![...]` or `let mut prompts = ...` with `PageCollector::new(request)` and reuse the same warning/dispatch failure block used for invalid cursor errors.

- [ ] **Step 3: Feed local resources through collector**

In `list_resources_impl`, change each local push/extend into `resources.accept(...)` and break local service iteration when `resources.finished()`:

```rust
resources.accept(
    Resource::new("lab://catalog", "catalog")
        .with_description("Full discovery document for all services")
        .with_mime_type("application/json"),
);

if !resources.finished()
    && code_mode_app_resources_visible(
        self.code_mode_visibility().await.exposes_synthetic_tools(),
        auth,
    )
{
    for resource in code_mode_app_resources() {
        resources.accept(resource);
        if resources.finished() {
            break;
        }
    }
}
```

- [ ] **Step 4: Feed upstream resources through collector**

For existing upstream `Vec<Resource>` returns, iterate and call `accept`, breaking between source groups when `resources.finished()` is true. Do not call additional subject-scoped resource lookups after the collector has a lookahead item.

- [ ] **Step 5: Feed prompts through collector**

In `list_prompts_impl`, create `builtin_prompts = crate::mcp::prompts::list_all().prompts`, keep `builtin_names` for upstream collision behavior, then iterate:

```rust
for prompt in builtin_prompts {
    prompts.accept(prompt);
    if prompts.finished() {
        break;
    }
}
```

Only call upstream prompt pool methods if `!prompts.finished()`.

- [ ] **Step 6: Finish collectors**

Replace old `paginate_items(...)` blocks with:

```rust
let (resources, next_cursor) = resources.finish();
```

and:

```rust
let (prompts, next_cursor) = prompts.finish();
```

- [ ] **Step 7: Run focused tests**

Run: `cargo test -p labby list_resources_paginates_large_builtin_catalog -- --nocapture`

Expected: PASS.

Run: `cargo test -p labby list_prompts_rejects_invalid_cursor -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/labby/src/mcp/handlers_resources.rs crates/labby/src/mcp/handlers_prompts.rs
git commit -m "perf(mcp): bound resources and prompts page collection"
```

### Task 4: Final Verification

**Files:**
- Modify: no new source expected unless verification finds issues.

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: green branch ready for PR.

- [ ] **Step 1: Format**

Run: `cargo fmt --all`

Expected: command exits 0.

- [ ] **Step 2: Run focused pagination tests**

Run these commands separately:

```bash
cargo test -p labby mcp::pagination::tests -- --nocapture
cargo test -p labby list_tools_paginates_large_builtin_catalog -- --nocapture
cargo test -p labby list_resources_paginates_large_builtin_catalog -- --nocapture
cargo test -p labby list_prompts_rejects_invalid_cursor -- --nocapture
```

Expected: all PASS.

- [ ] **Step 3: Run compile gate**

Run: `cargo check -p labby --all-features`

Expected: PASS.

- [ ] **Step 4: Run diff hygiene**

Run: `git diff --check`

Expected: no output.

- [ ] **Step 5: Commit any verification fixes**

If formatting or compile fixes changed files:

```bash
git add .
git commit -m "fix(mcp): finish pagination collector integration"
```

Expected: no commit if there were no additional changes.
