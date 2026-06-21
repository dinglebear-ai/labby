# Code Mode Elicitation Relay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an upstream MCP server's `elicitation/create` (and sampling/roots) raised during a Code Mode `callTool` reach the downstream agent and a human, instead of being silently declined — without the 30s Code Mode wall-clock killing the run while the human answers.

**Architecture:** Entirely parent-side. The Code Mode broker (`CodeModeBroker`) gains the originating `codemode` call's downstream `Peer<RoleServer>` + `relay_session_id` as fields, so the per-tool-call path (`execute.rs::call_tool_id`) can route through `UpstreamPool::call_tool_relayed` (which already forwards elicitation and uses the human-aware `relay_timeout`) instead of the pooled `pool.call_tool`. The **parent↔runner wire protocol is unchanged** — the runner just blocks longer awaiting its `tool_result`. Because of that block, `drive_runner`'s wall-clock deadline must be *suspended* while any relayed call is in flight, so human wait time does not count against the 30s compute budget.

**Tech Stack:** Rust 2024, tokio (`select!`, `FuturesUnordered`, `timeout_at`), rmcp (`Peer<RoleServer>`), the existing `relay.rs` machinery shipped in the prior PR.

## Global Constraints

- No `mod.rs` files; `foo.rs` sibling to `foo/`.
- `lab-apis` must not gain `rmcp`/`clap`/`axum`/`anyhow`. All work here is in the `lab` crate (`crates/lab`), which already depends on `rmcp`.
- Native `async fn in trait`; never `#[async_trait]`, never `Box<dyn ServiceClient>`.
- The relay path stays gated by `LAB_UPSTREAM_RELAY_ELICITATION` **and** a downstream-elicitation-capability check (`!peer.supported_elicitation_modes().is_empty()`) — identical to the direct-proxy gate in `mcp/call_tool_upstream.rs`.
- Relayed calls use the pool's `relay_timeout` (`upstream_relay_timeout_ms`, default 5 min) — **reuse the existing knob; do not add a new one.**
- Verify with the all-features path: `cargo nextest run -p labby --all-features` and `cargo clippy --workspace --all-features --all-targets`.
- Code Mode runs without a downstream peer (CLI / `search` / standalone tests) must behave exactly as today: pooled `call_tool`, elicitation declined.

---

## Background: the call chain (verified seams)

```
mcp/call_tool_codemode.rs::call_tool_codemode_impl(context)   <-- context.peer + self.relay_session_id live HERE
  └─ CodeModeBroker::new(&self.registry, Some(manager))        <-- peer dropped here today (line ~267)
       └─ broker.execute(code, caller, surface, config, filter)
            └─ execute_sandboxed(...)  →  run_in_runner(...)  →  run_in_runner_with_config(...)
                 └─ drive_runner(runner, cfg)                  <-- owns `deadline = now() + cfg.timeout` (runner_drive.rs:240)
                      └─ select! loop:
                           - timeout_at(deadline, lines.next())               <-- read-loop wall-clock
                           - ToolCall{seq,id,params} → enqueue_tool_call(..., deadline, ...)
                                └─ ToolCallFut = call_tool_id_before_deadline(deadline)   <-- per-call wall-clock
                                     └─ call_tool_id(...) → pool.call_tool(upstream, params)   (execute.rs:363)
```

Key facts that make this tractable:
- `CodeModeBroker`'s methods are all `impl CodeModeBroker<'_>` taking `&self`, so a new field is visible in `call_tool_id` **without threading any signatures**.
- `caller.oauth_subject()` already yields the subject for the relay cache key.
- The runner is oblivious to relaying; only `drive_runner`'s deadline handling needs to change.

---

## File Structure

| File | Responsibility / change |
|------|-------------------------|
| `crates/lab/src/dispatch/gateway/code_mode.rs` | `CodeModeBroker` struct def — add `downstream: Option<Peer<RoleServer>>`, `relay_session_id: u64` fields + update `new`. |
| `crates/lab/src/mcp/call_tool_codemode.rs` | Pass `context.peer.clone()` + `self.relay_session_id` into `CodeModeBroker::new`. |
| `crates/lab/src/dispatch/gateway/code_mode/execute.rs` | `call_tool_id`: route relay-eligible calls through `pool.call_tool_relayed`. New `relay_eligible()` helper. |
| `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs` | `drive_runner`: suspend the wall-clock while ≥1 relayed call is in flight; `enqueue_tool_call` branches relay-eligible calls off the `deadline` wrapper. |
| `crates/lab/src/dispatch/gateway/code_mode/CLAUDE.md` | Document Code Mode elicitation relay + wall-clock suspension. |
| `crates/lab/src/dispatch/upstream/pool/relay.rs` (module doc) & `crates/lab/src/mcp/CLAUDE.md` | Update the "Scope" note: relay now also covers Code Mode `callTool`. |
| `docs/dev/CODE_MODE.md` | Document the human-in-the-loop budget exception. |
| `CHANGELOG.md` | Unreleased entry. |

---

## Open decisions (resolve before/while executing)

1. **Concurrent elicitations from fan-out.** A snippet can fan out (`Promise.all([...])`) many `callTool`s; if several elicit, the agent receives several concurrent `elicitation/create` requests. The relay forwards each to the same downstream peer (rmcp multiplexes concurrent server→client requests). **Recommendation:** allow it (the agent/host decides how to present multiple prompts); document that fan-out + elicitation is the snippet author's responsibility. *Alternative:* cap concurrent in-flight relayed calls to 1 (serialize) — simpler UX, but changes Code Mode's concurrency contract. **Pick one before Task 3.**
2. **Wall-clock model.** Recommended: *suspend* (compute stays bounded at `code_mode.timeout_ms`; only human-wait time is excluded). *Alternative:* a separate larger fixed budget when relaying — simpler but lets a compute-heavy snippet that also elicits run longer than intended. Plan below implements **suspend**.
3. **Subject/OAuth in Code Mode.** Today `call_tool_id` uses non-subject-scoped `pool.call_tool`. The relay path will pass `caller.oauth_subject()`. Confirm Code Mode against OAuth upstreams is in scope; if OAuth-in-Code-Mode is out of scope, pass `None` and only non-OAuth upstreams relay. **Default: pass `caller.oauth_subject()`.**

---

### Task 1: Thread the downstream peer + session into `CodeModeBroker`

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` (struct `CodeModeBroker` + `fn new`)
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs:267` (the `new` call)
- Modify: every other `CodeModeBroker::new(...)` call site (search/CLI/tests) to pass `None, 0`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs` (compile-only; no behavior change)

**Interfaces:**
- Produces: `CodeModeBroker::new(registry, manager, downstream: Option<Peer<RoleServer>>, relay_session_id: u64)` and fields `self.downstream`, `self.relay_session_id`.

- [ ] **Step 1: Find all `CodeModeBroker::new` call sites**

Run: `rg -n 'CodeModeBroker::new' crates/lab/src/`
Expected: the MCP execute path (`call_tool_codemode.rs`), the search path, any CLI gateway code path, and test helpers.

- [ ] **Step 2: Add the fields to the struct**

In `code_mode.rs`, add to `struct CodeModeBroker<'a>`:

```rust
/// Downstream agent peer of the originating `codemode` MCP call, used to relay
/// an upstream's elicitation/sampling/roots to a human. `None` for CLI/standalone
/// Code Mode (no agent to forward to) — those keep the pooled, declining path.
downstream: Option<rmcp::service::Peer<rmcp::RoleServer>>,
/// Relay cache session id from the originating `LabMcpServer` session
/// (`next_relay_session_id()`), forming the relay cache key alongside the subject.
relay_session_id: u64,
```

- [ ] **Step 3: Update `CodeModeBroker::new`**

Add the two params to `new` and set the fields. Keep the existing `registry`/`gateway_manager` params first.

- [ ] **Step 4: Update all call sites**

`mcp/call_tool_codemode.rs:267`:
```rust
let broker = CodeModeBroker::new(
    &self.registry,
    Some(manager),
    Some(context.peer.clone()),
    self.relay_session_id,
);
```
Every other call site (search, CLI, tests): pass `None, 0`.

- [ ] **Step 5: Compile + existing tests pass (no behavior change yet)**

Run: `cargo nextest run -p labby --all-features -E 'test(/code_mode/) or test(/codemode/)'`
Expected: PASS (the fields are unused so far; expect a `dead_code`-style clippy note — acceptable until Task 2, or add `#[allow(dead_code)]` with a `// wired in Task 2` comment and remove it then).

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs crates/lab/src/mcp/call_tool_codemode.rs
git commit -m "refactor(code_mode): thread downstream peer + relay session into CodeModeBroker"
```

---

### Task 2: Route relay-eligible Code Mode tool calls through the relay

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/execute.rs` (`call_tool_id`, near the `pool.call_tool` at line ~363; add a `relay_eligible` helper)
- Test: `crates/lab/src/dispatch/gateway/code_mode/` (new `tests_relay.rs` or extend an existing test module)

**Interfaces:**
- Consumes: `self.downstream`, `self.relay_session_id`, `caller.oauth_subject()` (Task 1); `UpstreamPool::call_tool_relayed(config, subject, params, peer, session_id)` and `GatewayManager::upstream_config(name)` (existing).
- Produces: relayed elicitation behavior on the Code Mode path.

- [ ] **Step 1: Write the failing test — a fast elicitation reaches a mock agent**

Build a `CodeModeBroker` with a `downstream` peer wired to a mock agent that accepts elicitation (mirror `relay.rs::tests::AnsweringAgent` and the in-memory duplex harness), an in-process upstream whose tool elicits, set `LAB_UPSTREAM_RELAY_ELICITATION=1`, and assert the snippet's `callTool` result reflects acceptance. Keep the elicitation fast (well under the Code Mode timeout) so this test passes before Task 3.

> NOTE: this test needs the pool's relay path reachable from the broker. If wiring a full in-process upstream into the broker harness is heavy, gate this test `#[ignore]` and rely on a focused unit test of `relay_eligible()` + the branch selection; record that decision in the test module doc.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p labby --all-features -E 'test(/code_mode_relay/)'`
Expected: FAIL (currently routes through `pool.call_tool`, which declines elicitation → result shows decline).

- [ ] **Step 3: Add `relay_eligible` and branch in `call_tool_id`**

In `execute.rs`, before the `pool.call_tool(upstream, upstream_params)` call:

```rust
// Relay path (opt-in): when the originating codemode call carried a downstream
// agent that can elicit, and the operator enabled relaying, route this upstream
// call over a dedicated relay connection so an upstream elicitation reaches the
// agent instead of being declined by the pooled unit handler.
let relayed = if let Some(peer) = self.downstream.as_ref()
    && crate::config::env_flag_enabled("LAB_UPSTREAM_RELAY_ELICITATION")
    && !peer.supported_elicitation_modes().is_empty()
    && let Some(manager) = self.gateway_manager
    && let Some(config) = manager.upstream_config(upstream).await
{
    Some(
        pool.call_tool_relayed(
            &config,
            caller.oauth_subject(),
            upstream_params.clone(),
            peer.clone(),
            self.relay_session_id,
        )
        .await,
    )
} else {
    None
};
let call_outcome = match relayed {
    Some(Some(result)) => result,                 // relayed Some(Ok|Err)
    Some(None) => Err("relayed upstream connect failed".to_string()),
    None => match pool.call_tool(upstream, upstream_params).await {
        Some(result) => result,
        None => Err(format!("upstream `{upstream}` is not connected")),
    },
};
```

(Adapt to the exact existing `match pool.call_tool(...)` shape — preserve the current `Some/None` handling for the pooled branch. `caller`, `pool`, and `upstream` are all already in scope in `call_tool_id` — `caller: CodeModeCaller` is a parameter (see `execute.rs:274`), so `caller.oauth_subject()` is directly callable.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p labby --all-features -E 'test(/code_mode_relay/)'`
Expected: PASS — the fast elicitation is accepted via the relay.

- [ ] **Step 5: Regression — no peer ⇒ pooled/declined as before**

Add/run a test with `downstream: None`: assert the eliciting upstream call is declined (pooled path), proving CLI/standalone behavior is unchanged.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/execute.rs crates/lab/src/dispatch/gateway/code_mode/tests_relay.rs
git commit -m "feat(code_mode): relay upstream elicitation to the agent when gated"
```

---

### Task 3: Suspend the Code Mode wall-clock while a relayed call is in flight (RISK)

> This is the load-bearing task. The async `select!` restructure is the part most likely to need compiler iteration — implement it incrementally and lean on the slow-elicitation test below.

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs` (`drive_runner` loop, `enqueue_tool_call`, the `ToolCallFut` type)
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_relay.rs`

**Interfaces:**
- Consumes: `relay_eligible` predicate (Task 2).
- Produces: relayed-call wait time excluded from the 30s compute budget.

**Design:**
- Track `relayed_in_flight: usize` and `wall_clock_credit: Duration` in the loop.
- **Read-loop branch:** replace `timeout_at(deadline, lines.next())` with an effective deadline: `if relayed_in_flight > 0 { far_future } else { base_deadline + wall_clock_credit }`. While a human is answering, the runner sends nothing and the read-branch must not fire.
- **Per-call future:** in `enqueue_tool_call`, relay-eligible calls are **not** wrapped in `call_tool_id_before_deadline(deadline)` — they are bounded by the pool's `relay_timeout` internally. Increment `relayed_in_flight` when enqueuing a relay-eligible call; on completion, decrement and add its elapsed wall-time to `wall_clock_credit`. Non-relay calls keep the `deadline` wrapper unchanged.
- The relay-eligibility predicate used at enqueue time must be **cheap** (no `await`): `self.downstream.is_some() && env_flag_enabled(...) && !peer.supported_elicitation_modes().is_empty()`. The expensive config resolution still happens inside `call_tool_id`; if it fails there, the call falls back to pooled and finishes fast — which is fine (it was counted as relayed-in-flight but returns quickly).

- [ ] **Step 1: Write the failing test — a slow elicitation does NOT time out**

Set the broker's Code Mode timeout very short (e.g. `code_mode.timeout_ms = 200`) and the pool `relay_timeout` long (e.g. 5s). The mock agent delays its elicitation answer past 200ms (e.g. 600ms). Assert the snippet completes successfully (relayed elicitation accepted) rather than returning `"timeout"`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p labby --all-features -E 'test(/code_mode_relay_slow/)'`
Expected: FAIL with the `"timeout"` kind — the fixed `deadline` fires at 200ms.

- [ ] **Step 3: Restructure `drive_runner`'s deadline handling**

Introduce the counter + credit and the dynamic read deadline:

```rust
let base_deadline = tokio::time::Instant::now() + cfg.timeout;
let mut wall_clock_credit = Duration::ZERO;
let mut relayed_in_flight: usize = 0;
// far-future sentinel for "wall-clock suspended"
let suspended = tokio::time::Instant::now() + Duration::from_secs(86_400);
// ... in the loop, recompute each iteration:
let read_deadline = if relayed_in_flight > 0 { suspended } else { base_deadline + wall_clock_credit };
// select! { line = timeout_at(read_deadline, lines.next()) => { ... } , done = pending_tool_calls.next() => { ... } }
```

Make `enqueue_tool_call` (and `ToolCallFut`) carry whether a call is relay-eligible; for relay-eligible calls do not apply `call_tool_id_before_deadline` and have the completed future report its elapsed wall-time so the loop can `wall_clock_credit += elapsed; relayed_in_flight -= 1;` (and `relayed_in_flight += 1` at enqueue). Keep non-relay calls byte-identical to today.

- [ ] **Step 4: Run the slow test to verify it passes**

Run: `cargo nextest run -p labby --all-features -E 'test(/code_mode_relay_slow/)'`
Expected: PASS.

- [ ] **Step 5: Regression — pure-compute timeout still fires**

Add/confirm a test: a snippet that busy-waits (no relayed call) past `code_mode.timeout_ms` still returns `"timeout"`. This proves the compute budget is intact and the suspension only applies to relayed waits.

- [ ] **Step 6: Full relay + code_mode suites green**

Run: `cargo nextest run -p labby --all-features -E 'test(/relay/) or test(/code_mode/) or test(/codemode/)'`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs crates/lab/src/dispatch/gateway/code_mode/tests_relay.rs
git commit -m "feat(code_mode): exclude relayed-elicitation wait from the wall-clock budget"
```

---

### Task 4: Documentation + CHANGELOG

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/CLAUDE.md`, `crates/lab/src/dispatch/upstream/pool/relay.rs` (module "Scope" note), `crates/lab/src/mcp/CLAUDE.md`, `docs/dev/CODE_MODE.md`, `CHANGELOG.md`

- [ ] **Step 1: code_mode/CLAUDE.md** — add a section: Code Mode relays upstream elicitation to the originating agent when `downstream` is present + gated; the wall-clock is suspended during relayed waits (compute still bounded by `code_mode.timeout_ms`; relayed calls bounded by `relay_timeout`). No wire-protocol change.

- [ ] **Step 2: relay.rs module "Scope" + mcp/CLAUDE.md** — change the "call_tool only / Code Mode declines" note to "covers both the direct proxy `call_tool` and Code Mode `callTool`; resource/prompt fetches still decline." (This supersedes the scope limitation documented in the prior PR.)

- [ ] **Step 3: docs/dev/CODE_MODE.md** — document the human-in-the-loop budget exception under the Budget section, and that `timeout`/`"timeout"` still applies to compute time.

- [ ] **Step 4: CHANGELOG.md** — Unreleased entry under Added: "Code Mode now relays upstream elicitation to the agent and excludes human-answer time from the 30s wall-clock."

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: document Code Mode elicitation relay + wall-clock suspension"
```

---

## Self-Review

**Spec coverage:**
- Peer threading → Task 1. Relay routing → Task 2. Deadline suspension → Task 3. Docs → Task 4. ✅
- "No protocol change" — confirmed: the runner emits `tool_call` and awaits `tool_result` exactly as today; only the parent's servicing of that call (relay vs pool) and the loop's deadline change. ✅
- "No regression without a peer" — Task 2 Step 5 + Task 3 Step 5. ✅

**Placeholder scan:** Task 2/3 code blocks adapt to existing match shapes rather than re-printing the full surrounding function (the existing `pool.call_tool` `Some/None` handling and the `enqueue_tool_call`/`ToolCallFut` definitions must be read at execution time). This is deliberate — these are *modifications* to code the executor will have open — but it means **Task 2 and Task 3 require reading the current `execute.rs::call_tool_id` and `runner_drive.rs` bodies first**, called out in each task. Not a fabricated API; every symbol referenced (`call_tool_relayed`, `upstream_config`, `oauth_subject`, `env_flag_enabled`, `supported_elicitation_modes`, `call_tool_id_before_deadline`) exists today.

**Type consistency:** `CodeModeBroker::new(registry, manager, downstream, relay_session_id)` used consistently in Tasks 1–2; `relay_session_id: u64` matches `LabMcpServer::relay_session_id` and `call_tool_relayed`'s `session_id: u64`; `downstream: Option<Peer<RoleServer>>` matches `call_tool_relayed`'s `downstream: Peer<RoleServer>` (unwrapped at the call site).

**Risk callout:** Task 3's `select!`/`FuturesUnordered` restructure is the only non-mechanical change; everything else is field-threading and a branch. Budget the bulk of execution time and review there.

## Execution Handoff

Plan complete. Recommended: implement on a dedicated branch off `main` (not the current `claude/crazy-ride-fcc2d4`, which holds the direct-proxy PR), since this depends on that PR's `relay_timeout`/`call_tool_relayed` having merged.
