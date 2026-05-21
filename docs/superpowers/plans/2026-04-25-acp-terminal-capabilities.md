# ACP Terminal Capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ACP terminal display support first, then add full ACP terminal execution support only behind workspace jailing, process cleanup, output bounds, and tests.

**Architecture:** Phase 1 is metadata plumbing: Lab advertises codex/claude-compatible `_meta.terminal_output`, preserves ACP tool-call metadata, derives terminal output state in the chat event reducer, and reuses the existing terminal artifact UI. Phase 2 is a separate execution subsystem: a runtime-owned terminal manager implements ACP `terminal/*` requests, but only when workspace-root jailing and cleanup guarantees are in place.

**Tech Stack:** Rust 2024, `agent-client-protocol`, Tokio process management, Next.js/TypeScript chat UI, Node test runner, ESLint, Cargo tests.

---

## Scope

Phase 1 must not enable `clientCapabilities.terminal = true`. It only enables display-terminal metadata for agents that already execute commands themselves, matching `../acp/codex-acp` and `../acp/claude-agent-acp`.

Phase 2 may enable `clientCapabilities.terminal = true`, but only after the terminal manager passes tests for jailing, process groups, output limits, kill/release behavior, wait behavior, and runtime shutdown cleanup.

## File Structure

- Modify: `crates/lab/src/acp/runtime.rs`
  - Advertise `_meta.terminal_output`.
  - Preserve `ToolCall.meta` and `ToolCallUpdate.meta`.
  - Later, wire terminal request handlers to a terminal manager.

- Create: `crates/lab/src/acp/terminal.rs`
  - Runtime-owned terminal manager for Phase 2.
  - Owns terminal IDs, spawned processes, bounded output buffers, exit status, and cleanup.

- Modify: `crates/lab/src/acp.rs`
  - Export `terminal` module after Phase 2 file exists.

- Modify: `crates/lab/src/acp/types.rs` and `crates/lab-apis/src/acp/types.rs`
  - Add optional metadata fields only if the current `ProviderInfo`/raw event route is insufficient.
  - Prefer preserving metadata under existing raw objects in Phase 1 to avoid API churn.

- Modify: `apps/gateway-admin/lib/acp/types.ts`
  - Add typed terminal metadata shapes if useful for reducer clarity.
  - Keep raw ACP metadata available.

- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
  - Extract `_meta.terminal_info`, `_meta.terminal_output`, and `_meta.terminal_exit`.
  - Merge terminal output into the relevant `TranscriptToolCall`.

- Modify: `apps/gateway-admin/components/chat/types.ts`
  - Add terminal fields to `TranscriptToolCall` if existing `content`/`output` fields cannot represent streaming terminal state clearly.

- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.ts`
  - Prefer terminal metadata output when deriving terminal artifacts.

- Modify: `apps/gateway-admin/components/chat/tool-artifact-panels.tsx`
  - Reuse existing terminal artifact panel; only adjust props if terminal metadata needs exit-status display.

- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`
  - Unit coverage for codex-acp and claude-agent-acp metadata sequences.

- Test: `apps/gateway-admin/components/chat/tool-call-presentation.test.ts`
  - Unit coverage that metadata-derived terminal output renders as terminal artifact.

- Test: `crates/lab/src/acp/runtime.rs` unit tests
  - Verify metadata is preserved from ACP tool calls/updates.
  - Verify initialize client capabilities include `_meta.terminal_output` and do not set `terminal=true` in Phase 1.

- Test: `crates/lab/src/acp/terminal.rs` unit tests
  - Phase 2 terminal manager tests.

---

## Phase 1: Display-Terminal Metadata

### Task 1: Preserve ACP Tool Metadata In Backend Events

**Files:**
- Modify: `crates/lab/src/acp/runtime.rs`

- [ ] **Step 1: Write failing Rust tests for metadata preservation**

Add tests near existing `acp::runtime::tests`:

```rust
#[test]
fn tool_call_metadata_preserves_terminal_info() {
    let mut tool_call = ToolCall::new("tool-1", "Run tests");
    tool_call = tool_call.meta(agent_client_protocol::schema::Meta::from_iter([(
        "terminal_info".to_string(),
        serde_json::json!({ "terminal_id": "term-1", "cwd": "/tmp/work" }),
    )]));

    let value = tool_call_metadata_payload(&tool_call);

    assert_eq!(
        value.get("_meta").and_then(|meta| meta.get("terminal_info")),
        Some(&serde_json::json!({ "terminal_id": "term-1", "cwd": "/tmp/work" }))
    );
}

#[test]
fn tool_call_update_output_preserves_terminal_output_meta() {
    let update = agent_client_protocol::schema::ToolCallUpdate::new(
        "tool-1",
        agent_client_protocol::schema::ToolCallUpdateFields::new(),
    )
    .meta(agent_client_protocol::schema::Meta::from_iter([(
        "terminal_output".to_string(),
        serde_json::json!({ "terminal_id": "term-1", "data": "cargo check\n" }),
    )]));

    let value = tool_call_update_output(&update.fields, update.meta.as_ref());

    assert_eq!(
        value.get("_meta").and_then(|meta| meta.get("terminal_output")),
        Some(&serde_json::json!({ "terminal_id": "term-1", "data": "cargo check\n" }))
    );
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::tool_call --all-features --bin lab`

Expected: FAIL because helper functions or `_meta` preservation do not exist.

- [ ] **Step 3: Extract backend metadata helpers**

In `crates/lab/src/acp/runtime.rs`, add focused helpers:

```rust
fn meta_to_value(meta: Option<&agent_client_protocol::schema::Meta>) -> Value {
    meta.and_then(|meta| serde_json::to_value(meta).ok())
        .unwrap_or(Value::Null)
}

fn tool_call_metadata_payload(tool_call: &agent_client_protocol::schema::ToolCall) -> Value {
    json!({
        "type": "tool_call_metadata",
        "tool_call_id": tool_call.tool_call_id.to_string(),
        "title": tool_call.title.clone(),
        "tool_kind": enum_value(&tool_call.kind),
        "status": enum_value(&tool_call.status),
        "locations": tool_call.locations.iter().map(|location| location.path.display().to_string()).collect::<Vec<_>>(),
        "content": tool_call.content.clone(),
        "raw_output": tool_call.raw_output.clone(),
        "_meta": meta_to_value(tool_call.meta.as_ref()),
    })
}
```

Change the existing `SessionUpdate::ToolCall` provider-info payload to call `tool_call_metadata_payload(&tool_call)`.

Change `tool_call_update_output` signature:

```rust
fn tool_call_update_output(
    fields: &agent_client_protocol::schema::ToolCallUpdateFields,
    meta: Option<&agent_client_protocol::schema::Meta>,
) -> Value
```

Include `_meta` in both raw and structured outputs:

```rust
if let Some(raw_output) = fields.raw_output.clone() {
    return json!({
        "raw_output": raw_output,
        "_meta": meta_to_value(meta),
    });
}

json!({
    "title": fields.title.clone(),
    "kind": fields.kind.as_ref().and_then(enum_value),
    "status": fields.status.as_ref().and_then(enum_value),
    "content": fields.content.clone(),
    "locations": fields.locations.as_ref().map(|locations| {
        locations.iter().map(|location| location.path.display().to_string()).collect::<Vec<_>>()
    }),
    "raw_input": fields.raw_input.clone(),
    "_meta": meta_to_value(meta),
})
```

Update the call site:

```rust
output: tool_call_update_output(&update.fields, update.meta.as_ref()),
```

- [ ] **Step 4: Run backend tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::tool_call --all-features --bin lab`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/acp/runtime.rs
git commit -m "feat(acp): preserve tool terminal metadata"
```

### Task 2: Advertise Display-Terminal Metadata Without Terminal Execution

**Files:**
- Modify: `crates/lab/src/acp/runtime.rs`

- [ ] **Step 1: Write failing test for initialize capabilities**

Add a test for a helper that constructs client capabilities:

```rust
#[test]
fn client_capabilities_enable_terminal_output_metadata_only() {
    let capabilities = lab_client_capabilities();
    let value = serde_json::to_value(&capabilities).unwrap();

    assert_eq!(value.get("terminal"), Some(&serde_json::json!(false)));
    assert_eq!(
        value.get("_meta").and_then(|meta| meta.get("terminal_output")),
        Some(&serde_json::json!(true))
    );
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::client_capabilities --all-features --bin lab`

Expected: FAIL because helper does not exist.

- [ ] **Step 3: Add helper and use it in initialize**

In `runtime.rs` imports, ensure `Meta` is available:

```rust
use agent_client_protocol::schema::Meta;
```

Add:

```rust
fn lab_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(
            FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true),
        )
        .meta(Meta::from_iter([(
            "terminal_output".to_string(),
            serde_json::json!(true),
        )]))
}
```

Replace the inline initialize capability builder with:

```rust
.client_capabilities(lab_client_capabilities())
```

Do not call `.terminal(true)` in this task.

- [ ] **Step 4: Run backend tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::client_capabilities --all-features --bin lab`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/acp/runtime.rs
git commit -m "feat(acp): advertise terminal output metadata"
```

### Task 3: Derive Streaming Terminal State In Chat Events

**Files:**
- Modify: `apps/gateway-admin/components/chat/types.ts`
- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`

- [ ] **Step 1: Write failing reducer tests for codex-acp metadata**

Add test:

```ts
test('deriveTranscriptAndActivity merges terminal metadata into tool calls', () => {
  const derived = deriveTranscriptAndActivity([
    event(1, {
      kind: 'tool.call',
      toolCallId: 'tool-1',
      title: 'Run tests',
      rawInput: { command: 'cargo test' },
    }),
    event(2, {
      kind: 'debug',
      raw: {
        type: 'tool_call_metadata',
        tool_call_id: 'tool-1',
        content: [{ type: 'terminal', terminalId: 'term-1' }],
        _meta: { terminal_info: { terminal_id: 'term-1', cwd: '/repo' } },
      },
    }),
    event(3, {
      kind: 'tool.update',
      toolCallId: 'tool-1',
      rawOutput: {
        _meta: { terminal_output: { terminal_id: 'term-1', data: 'running\\n' } },
      },
    }),
    event(4, {
      kind: 'tool.update',
      toolCallId: 'tool-1',
      status: 'completed',
      rawOutput: {
        _meta: { terminal_exit: { terminal_id: 'term-1', exit_code: 0, signal: null } },
      },
    }),
  ]);

  const toolCall = derived.messages.at(-1)?.toolCalls[0];
  assert.equal(toolCall?.terminal?.terminalId, 'term-1');
  assert.equal(toolCall?.terminal?.output, 'running\\n');
  assert.equal(toolCall?.terminal?.exitCode, 0);
  assert.equal(toolCall?.status, 'completed');
});
```

- [ ] **Step 2: Run test to verify failure**

Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/chat/session-events.test.ts`

Expected: FAIL because `terminal` does not exist on `TranscriptToolCall`.

- [ ] **Step 3: Add terminal model types**

In `apps/gateway-admin/components/chat/types.ts`:

```ts
export interface TranscriptTerminal {
  terminalId: string
  cwd?: string
  output: string
  exitCode?: number | null
  signal?: string | null
}

export interface TranscriptToolCall {
  // existing fields...
  terminal?: TranscriptTerminal | null
}
```

- [ ] **Step 4: Add metadata extraction helpers**

In `session-events.ts`, add helpers:

```ts
type TerminalPatch = {
  terminalId: string
  cwd?: string
  appendOutput?: string
  exitCode?: number | null
  signal?: string | null
}

function readMeta(raw: unknown): Record<string, unknown> | null {
  if (!isRecord(raw)) return null
  const meta = raw._meta
  return isRecord(meta) ? meta : null
}

function readTerminalPatch(raw: unknown): TerminalPatch | null {
  const meta = readMeta(raw)
  if (!meta) return null

  const info = meta.terminal_info
  if (isRecord(info) && typeof info.terminal_id === 'string') {
    return {
      terminalId: info.terminal_id,
      cwd: typeof info.cwd === 'string' ? info.cwd : undefined,
    }
  }

  const output = meta.terminal_output
  if (isRecord(output) && typeof output.terminal_id === 'string') {
    return {
      terminalId: output.terminal_id,
      appendOutput: typeof output.data === 'string' ? output.data : '',
    }
  }

  const exit = meta.terminal_exit
  if (isRecord(exit) && typeof exit.terminal_id === 'string') {
    return {
      terminalId: exit.terminal_id,
      exitCode: typeof exit.exit_code === 'number' ? exit.exit_code : null,
      signal: typeof exit.signal === 'string' ? exit.signal : null,
    }
  }

  return null
}
```

Extend `ToolCallPatch` with `terminal?: TerminalPatch`.

- [ ] **Step 5: Merge terminal patch in tool calls**

In `toolPatchFromEvent`, add terminal patch extraction from:
- `event.raw` for `tool_call_metadata`
- `event.rawOutput` for `tool.update`

In `upsertToolCall`, merge:

```ts
const previousTerminal = previous?.terminal ?? null
const terminalPatch = patch.terminal
const terminal = terminalPatch
  ? {
      terminalId: terminalPatch.terminalId,
      cwd: terminalPatch.cwd ?? previousTerminal?.cwd,
      output: `${previousTerminal?.output ?? ''}${terminalPatch.appendOutput ?? ''}`,
      exitCode: terminalPatch.exitCode ?? previousTerminal?.exitCode,
      signal: terminalPatch.signal ?? previousTerminal?.signal,
    }
  : previousTerminal
```

Set `terminal` on the returned `TranscriptToolCall`.

- [ ] **Step 6: Run reducer tests**

Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/chat/session-events.test.ts`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/chat/types.ts apps/gateway-admin/lib/chat/session-events.ts apps/gateway-admin/lib/chat/session-events.test.ts
git commit -m "feat(chat): merge ACP terminal metadata"
```

### Task 4: Render Metadata-Derived Terminal Output

**Files:**
- Modify: `apps/gateway-admin/components/chat/tool-call-presentation.ts`
- Modify: `apps/gateway-admin/components/chat/tool-artifact-panels.tsx` only if needed
- Test: `apps/gateway-admin/components/chat/tool-call-presentation.test.ts`

- [ ] **Step 1: Write failing presentation test**

Add test:

```ts
test('getInlineArtifact prefers streamed terminal metadata output', () => {
  const artifact = getInlineArtifact(
    toolCall({
      terminal: {
        terminalId: 'term-1',
        cwd: '/repo',
        output: 'pnpm test\\npass\\n',
        exitCode: 0,
        signal: null,
      },
      output: null,
      content: [{ type: 'terminal', terminalId: 'term-1' }],
    }),
  )

  assert.equal(artifact.kind, 'terminal')
  assert.equal(artifact.terminalOutput, 'pnpm test\\npass\\n')
})
```

- [ ] **Step 2: Run test to verify failure**

Run: `cd apps/gateway-admin && pnpm exec tsx --test components/chat/tool-call-presentation.test.ts`

Expected: FAIL if metadata terminal is ignored.

- [ ] **Step 3: Prefer `toolCall.terminal.output` in presentation**

In `getInlineArtifact`, before fallback text flattening:

```ts
if (toolCall.terminal?.output) {
  return {
    kind: 'terminal',
    title: toolCall.title,
    terminalOutput: toolCall.terminal.output,
    summary: toolCall.terminal.exitCode === undefined ? 'Running' : `Exited ${toolCall.terminal.exitCode ?? 'signal'}`,
  }
}
```

Adjust exact fields to match existing `ToolArtifact` type.

- [ ] **Step 4: Run presentation tests**

Run: `cd apps/gateway-admin && pnpm exec tsx --test components/chat/tool-call-presentation.test.ts`

Expected: PASS.

- [ ] **Step 5: Run frontend targeted tests/lint**

Run:

```bash
cd apps/gateway-admin
pnpm exec eslint lib/acp/types.ts lib/chat/session-events.ts lib/chat/session-events.test.ts components/chat/types.ts components/chat/tool-call-presentation.ts components/chat/tool-call-presentation.test.ts components/chat/tool-artifact-panels.tsx
pnpm exec tsx --test lib/chat/session-events.test.ts components/chat/tool-call-presentation.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/components/chat/tool-call-presentation.ts apps/gateway-admin/components/chat/tool-artifact-panels.tsx apps/gateway-admin/components/chat/tool-call-presentation.test.ts
git commit -m "feat(chat): render ACP terminal output"
```

### Task 5: Phase 1 End-To-End Verification

**Files:**
- No code changes unless verification fails.

- [ ] **Step 1: Run backend focused tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 2: Run frontend focused tests**

Run:

```bash
cd apps/gateway-admin
pnpm exec eslint lib/acp/types.ts lib/chat/session-events.ts lib/chat/session-events.test.ts components/chat/types.ts components/chat/tool-call-presentation.ts components/chat/tool-call-presentation.test.ts components/chat/tool-artifact-panels.tsx
pnpm exec tsx --test lib/chat/session-events.test.ts components/chat/tool-call-presentation.test.ts
```

Expected: PASS.

- [ ] **Step 3: Manual smoke with codex-acp**

Run Lab with rebuilt web assets and start a chat prompt that triggers a shell command.

Expected:
- Tool call shows one tool card.
- Terminal output streams into that tool card.
- Exit status marks completion.
- Session still closes after stop reason or clean provider EOF.

- [ ] **Step 4: Commit verification notes if docs are updated**

Only commit docs if new notes are added:

```bash
git add docs/acp
git commit -m "docs(acp): document terminal output metadata"
```

---

## Phase 2: Full ACP Terminal Execution Capability

### Task 6: Design Terminal Manager API With Tests First

**Files:**
- Create: `crates/lab/src/acp/terminal.rs`
- Modify: `crates/lab/src/acp.rs`

- [ ] **Step 1: Write terminal manager tests**

Create tests inside `terminal.rs`:

```rust
#[tokio::test]
async fn terminal_manager_rejects_cwd_outside_workspace() {
    let manager = TerminalManager::new(tempfile::tempdir().unwrap().path().to_path_buf());
    let error = manager
        .create(CreateTerminalSpec {
            session_id: "s".to_string(),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "echo nope".to_string()],
            env: vec![],
            cwd: Some(std::path::PathBuf::from("/")),
            output_byte_limit: None,
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("outside workspace"));
}

#[tokio::test]
async fn terminal_manager_captures_output_and_exit() {
    let root = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new(root.path().to_path_buf());
    let id = manager
        .create(CreateTerminalSpec {
            session_id: "s".to_string(),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "printf hello".to_string()],
            env: vec![],
            cwd: Some(root.path().to_path_buf()),
            output_byte_limit: None,
        })
        .await
        .unwrap();

    let exit = manager.wait_for_exit(&id).await.unwrap();
    let output = manager.output(&id).await.unwrap();

    assert_eq!(exit.exit_code, Some(0));
    assert_eq!(output.output, "hello");
    assert!(!output.truncated);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::terminal::tests:: --all-features --bin lab`

Expected: FAIL because module does not exist.

- [ ] **Step 3: Implement minimal terminal manager types**

Create `crates/lab/src/acp/terminal.rs` with:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};

pub struct TerminalManager {
    workspace_root: PathBuf,
    terminals: Mutex<HashMap<String, Arc<ManagedTerminal>>>,
}

pub struct CreateTerminalSpec {
    pub session_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub output_byte_limit: Option<u64>,
}

pub struct TerminalOutputSnapshot {
    pub output: String,
    pub truncated: bool,
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

pub struct TerminalExitSnapshot {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}
```

Implement only enough for tests, using `tokio::process::Command`, stdout/stderr pipes, a shared output buffer, and `Notify` for completion.

- [ ] **Step 4: Export module**

In `crates/lab/src/acp.rs`:

```rust
mod terminal;
```

Use `pub(crate)` visibility as needed.

- [ ] **Step 5: Run terminal manager tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::terminal::tests:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/acp.rs crates/lab/src/acp/terminal.rs
git commit -m "feat(acp): add terminal manager"
```

### Task 7: Add Output Bounds, UTF-8 Safe Truncation, Kill, Release, Cleanup

**Files:**
- Modify: `crates/lab/src/acp/terminal.rs`

- [ ] **Step 1: Write failing tests**

Add tests:
- `terminal_manager_truncates_from_start_at_char_boundary`
- `terminal_manager_kill_preserves_terminal_for_final_output`
- `terminal_manager_release_invalidates_terminal`
- `terminal_manager_shutdown_releases_all_processes`

Use shell commands that are stable on Linux:

```rust
sh -c "printf 'αβγδεζηθικ'"
sh -c "sleep 30"
```

- [ ] **Step 2: Run tests to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::terminal::tests:: --all-features --bin lab`

Expected: FAIL for unimplemented behaviors.

- [ ] **Step 3: Implement bounded buffer and lifecycle**

Requirements:
- Maintain output as bytes internally.
- On append, trim from the front until `len <= output_byte_limit`.
- Convert to string with `String::from_utf8_lossy`, but never split retained bytes in the middle of a UTF-8 character. Prefer storing a `String` and trimming by `char_indices`.
- `kill` terminates the process but leaves the terminal record.
- `release` kills if needed and removes the record.
- `shutdown` releases all terminals.

- [ ] **Step 4: Add Unix process-group cleanup**

On Unix:
- Spawn child in a process group.
- `kill` and `release` target the process group.
- Fall back to `child.kill()` if process-group setup fails.

- [ ] **Step 5: Run tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::terminal::tests:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/acp/terminal.rs
git commit -m "feat(acp): harden terminal lifecycle"
```

### Task 8: Wire Terminal Manager Into ACP Runtime Without Advertising Yet

**Files:**
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `crates/lab/src/acp/terminal.rs`

- [ ] **Step 1: Write handler tests or narrow integration test**

If direct JSON-RPC handler testing is hard, add unit tests for conversion functions:

```rust
#[test]
fn create_terminal_spec_converts_acp_request() {
    let request = CreateTerminalRequest::new("s", "sh")
        .args(vec!["-c".to_string(), "echo hi".to_string()])
        .cwd(Some(PathBuf::from("/repo")))
        .output_byte_limit(Some(1024));

    let spec = create_terminal_spec_from_request(request);

    assert_eq!(spec.command, "sh");
    assert_eq!(spec.output_byte_limit, Some(1024));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::create_terminal_spec --all-features --bin lab`

Expected: FAIL.

- [ ] **Step 3: Instantiate terminal manager**

In `run_codex_session`, create:

```rust
let terminal_manager = Arc::new(TerminalManager::new(input.cwd.clone()));
```

Pass clones into terminal request handlers.

- [ ] **Step 4: Replace terminal `method_not_found` handlers**

Map:
- `CreateTerminalRequest` -> `TerminalManager::create` -> `CreateTerminalResponse::new(id)`
- `TerminalOutputRequest` -> `TerminalManager::output` -> `TerminalOutputResponse::new(...).exit_status(...)`
- `WaitForTerminalExitRequest` -> `TerminalManager::wait_for_exit` -> `WaitForTerminalExitResponse::new(...)`
- `KillTerminalRequest` -> `TerminalManager::kill` -> `KillTerminalResponse::new()`
- `ReleaseTerminalRequest` -> `TerminalManager::release` -> `ReleaseTerminalResponse::new()`

Convert manager errors to ACP `invalid_params` for bad cwd/unknown terminal and `internal_error` for process failures.

- [ ] **Step 5: Ensure runtime shutdown cleans terminals**

Before `run_codex_session` exits:

```rust
terminal_manager.shutdown().await;
```

- [ ] **Step 6: Run backend tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/acp/runtime.rs crates/lab/src/acp/terminal.rs
git commit -m "feat(acp): handle terminal requests"
```

### Task 9: Advertise Full Terminal Capability Behind Explicit Safety Gate

**Files:**
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `docs/acp/TERMINALS.md` or create if absent

- [ ] **Step 1: Write failing capability-gate tests**

Add tests:

```rust
#[test]
fn client_capabilities_do_not_enable_terminal_by_default() {
    let capabilities = lab_client_capabilities(false);
    let value = serde_json::to_value(&capabilities).unwrap();
    assert_eq!(value.get("terminal"), Some(&serde_json::json!(false)));
}

#[test]
fn client_capabilities_enable_terminal_when_safety_gate_is_true() {
    let capabilities = lab_client_capabilities(true);
    let value = serde_json::to_value(&capabilities).unwrap();
    assert_eq!(value.get("terminal"), Some(&serde_json::json!(true)));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests::client_capabilities --all-features --bin lab`

Expected: FAIL until helper accepts gate.

- [ ] **Step 3: Add explicit gate**

Use an env var or config flag. Recommended:

```rust
fn acp_terminal_execution_enabled() -> bool {
    std::env::var("LAB_ACP_ENABLE_TERMINAL_EXECUTION")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}
```

Only call `.terminal(true)` when this is true and workspace root is configured.

- [ ] **Step 4: Document the gate**

Create `docs/acp/TERMINALS.md`:

```markdown
# ACP Terminals

Lab supports two terminal-related ACP paths:

- Display terminal metadata: enabled by default with `_meta.terminal_output=true`; agents execute their own commands and Lab renders streamed terminal output.
- Terminal execution: disabled by default; enable only with `LAB_ACP_ENABLE_TERMINAL_EXECUTION=true` after workspace jailing is configured.

Terminal execution is workspace-jailed and process-group cleaned up on kill, release, and runtime shutdown.
```

- [ ] **Step 5: Run tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/acp/runtime.rs docs/acp/TERMINALS.md
git commit -m "feat(acp): gate terminal execution capability"
```

### Task 10: Phase 2 End-To-End Verification

**Files:**
- No code changes unless verification fails.

- [ ] **Step 1: Run backend ACP tests**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp:: --all-features --bin lab`

Expected: PASS.

- [ ] **Step 2: Run full binary build**

Run: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo build --manifest-path crates/lab/Cargo.toml --all-features --bin lab`

Expected: PASS.

- [ ] **Step 3: Manual smoke with terminal disabled**

Run Lab without `LAB_ACP_ENABLE_TERMINAL_EXECUTION`.

Expected:
- Initialize advertises `_meta.terminal_output=true`.
- Initialize does not advertise `terminal=true`.
- codex-acp/claude-agent-acp display terminal output still works.

- [ ] **Step 4: Manual smoke with terminal enabled**

Run Lab with:

```bash
LAB_ACP_ENABLE_TERMINAL_EXECUTION=true
```

Use a test ACP agent that calls `terminal/create`, `terminal/output`, `terminal/wait_for_exit`, and `terminal/release`.

Expected:
- Command runs only inside workspace root.
- Output is retrievable.
- Exit status is correct.
- Release invalidates terminal ID.
- Runtime shutdown kills lingering processes.

- [ ] **Step 5: Final commit or fix failures**

If manual smoke required fixes:

```bash
git add crates/lab/src/acp/runtime.rs crates/lab/src/acp/terminal.rs docs/acp/TERMINALS.md
git commit -m "fix(acp): stabilize terminal execution smoke"
```

---

## Verification Matrix

- Backend display metadata: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::runtime::tests:: --all-features --bin lab`
- Backend terminal manager: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo test --manifest-path crates/lab/Cargo.toml acp::terminal::tests:: --all-features --bin lab`
- Frontend reducer: `cd apps/gateway-admin && pnpm exec tsx --test lib/chat/session-events.test.ts`
- Frontend artifact rendering: `cd apps/gateway-admin && pnpm exec tsx --test components/chat/tool-call-presentation.test.ts`
- Frontend lint: `cd apps/gateway-admin && pnpm exec eslint lib/chat/session-events.ts lib/chat/session-events.test.ts components/chat/types.ts components/chat/tool-call-presentation.ts components/chat/tool-call-presentation.test.ts components/chat/tool-artifact-panels.tsx`
- Binary build: `CARGO_TARGET_DIR=/tmp/lab-acp-terminal-target cargo build --manifest-path crates/lab/Cargo.toml --all-features --bin lab`

## References

- Local schema: `../acp/agent-client-protocol/src/client.rs`
- Rust SDK migration: `../acp/rust-sdk/md/migration_v0.11.x.md`
- codex-acp terminal metadata: `../acp/codex-acp/src/thread.rs`
- claude-agent-acp terminal metadata: `../acp/claude-agent-acp/src/acp-agent.ts`
- Official docs: https://agentclientprotocol.com/protocol/terminals
- Schema source: https://docs.rs/agent-client-protocol-schema/latest/src/agent_client_protocol_schema/client.rs.html

