# Lab Serve Observability and Formatter Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

---

## Engineering Review (2026-04-21)

> Reviewed by: architecture-strategist, code-simplicity-reviewer, security-sentinel, performance-oracle

### Architecture

**Strengths:** Core decomposition is sound — tracing events remain the semantic source of truth, presentation is upgraded in isolation, `lab-apis` untouched. Test-before-replace and data-first field ordering in Task 3 both correct.

**Concerns:**
- `main.rs` SRP violated — formatter is already the majority of the file and Task 1 worsens it. **Extract into `crates/lab/src/tracing/` (formatter.rs, style.rs, categories.rs) as part of Task 1, not later.**
- 49-entry priority list + new semantic coloring = two-edit-per-field pattern going forward. Introduce `FieldCategory` enum + `field_category(key: &str) -> FieldCategory` in new `categories.rs` to drive both ordering and coloring from one place.
- Vestigial `with_ansi(ansi)` call in `init_tracing()` is a no-op with the custom formatter — remove it or document the no-op.
- Verify `LogIngestLayer` (`crates/lab/src/dispatch/logs/ingest.rs`) does not pattern-match on `subsystem`/`phase` string values that Task 2 renames. A string rename there breaks ingest silently.

### Simplicity

**Over-engineering to remove:**
- **Do not add `console` crate.** `owo-colors` is already in `Cargo.toml`. The existing `style(text, code)` pattern achieves the same result with ~4 new one-liner methods. `console` adds terminal capability detection that can diverge from the existing `ansi: bool` gate (see Performance failure mode).
- **Remove shell wrapper modification entirely.** `/home/jmagar/.cargo/bin/lab` is a `cargo install` artifact — overwritten on every reinstall, invisible to CI, inaccessible to contributors. Port any preflight logic to `serve::run()` as a `subsystem = "cli-preflight"` phase in Rust, or delete the plan item.
- **Defer formatter assertion test.** Testing `PremiumEventFormatter` requires constructing a full tracing subscriber stack — non-trivial boilerplate for a binary crate. Only unit-test the pure helpers (`normalize_label`, `format_field_value`, `should_skip_field`). The formatter correctness is verified by the manual Task 4 checklist.
- **Trim priority field list** from 30+ to ~10 runtime-relevant fields. Startup-only fields (`session_ttl_secs`, `web_ui_auth_disabled`, `http_mcp_enabled`) fall naturally to the catch-all alphabetical loop.

**Simplifications:**
- Remove ~60 LOC of redundant "startup state resolved" `tracing::info!` calls in `run_http()` — they duplicate the final `phase = "ready"` summary immediately below them.
- Change `INFO` badge from `"1;36"` (bold cyan) to `"32"` (green) to match Axon palette — 1 character.
- Axon palette maps directly to ANSI codes: accent=`"38;5;111"`, subject=`"38;5;211"`, subtle=`"38;5;110"`. No library needed.

### Security

**HIGH — Must fix before implementing Task 3:**

1. **ANSI escape injection via untrusted upstream field values.** `record_str` (the `%field` Display path) stores values raw with no control-char escaping. `format_field_value()` only quotes whitespace — ANSI sequences contain no whitespace. A malicious upstream can register a tool named `\x1b[2J\x1b[H\x1b[31mFAKE CRITICAL\x1b[0m`. Adding `console::Style` wrappers makes this *worse* — the reset boundary becomes predictable and injection more effective. **Fix: add `sanitize_field_value()` that replaces C0 controls (0x00–0x1F except tab/newline) with `\u{FFFD}`, applied in `format_field_value()` and on the message field, before any styling.**

2. **URL credentials leak in `upstream_target()`.** `pool.rs` logs `config.url.clone()` verbatim at three discovery event sites. `https://admin:s3cr3t@internal-mcp.example.com` is a valid URL that passes all current validation. OBSERVABILITY.md §"Required Fields" forbids logging credential-bearing URLs. **Fix: replace `upstream_target()` with a redacting variant that calls `parsed.set_username("")` and `parsed.set_password(None)` before converting to string. Also add a validation error in `config.rs` when userinfo is present.**

3. **`resource_uri` field leaks query-string credentials.** Task 3 adds `resource_uri` as a logged field. Upstream MCP servers can return resource URIs with pre-signed AWS tokens or OAuth params in query strings. **Fix: add `redact_resource_uri_for_logging()` that strips from `?` or `#` onward before any `resource_uri` is logged. Apply at every new event site in Task 3.**

**MEDIUM:**
- Shell wrapper `cat "$build_log" >&2` on pnpm failure can dump private npm registry auth tokens embedded in error URLs. If the wrapper is kept (not recommended), filter the output before emitting.
- New startup auth-summary events have no code-level field whitelist. Add a `// SECURITY: Only log metadata — never resolved secret values` comment adjacent to every new startup event site in Task 2 and existing ones in `serve.rs:149-156`.

### Performance

**Block on:**
- Do not add `console`. In Docker/piped environments, `console::colors_enabled()` checks `COLORTERM` env var and can return `true` while `human_logs_use_ansi()` returns `false` (pipe = not a TTY). This produces ANSI codes in machine-consumed log streams, breaking aggregator regex parsing. The existing single `ansi: bool` gate is correct. Keep it.

**Do in same pass:**
- Change `BTreeMap<String, String>` to `BTreeMap<&'static str, String>` in `EventFieldCollector`. The `Field::name()` API already returns `&'static str` — the key allocation is unnecessary. Eliminates 5–12 heap allocations per log event for free.
- Ensure all new startup lifecycle events in Task 2 are placed **after** the `init_tracing()` call in `main.rs`. Events emitted before the subscriber is registered are silently dropped with no error — a hard-to-diagnose failure that looks like a logging bug.

**Deferrable:** SmallVec for fields (breaks alphabetical straggler ordering), phf::Map for priority lookup (add only if profiler shows formatter in hot path at scale), exact color256 tuning.

### Failure Modes

```
CODEPATH                          | FAILURE MODE                                      | RESCUED? | TEST? | USER SEES?    | LOGGED?
----------------------------------|---------------------------------------------------|----------|-------|---------------|--------
Task 1: ANSI formatter styling    | Untrusted tool name injects ANSI → forged log     | N        | N     | Silent/forged | N
Task 1: console::Style adoption   | Docker pipe: console=true, ansi=false → ANSI bleed | N        | N     | Visible/break | Y (broken)
Task 2: startup lifecycle events  | Event emitted before init_tracing() → silent drop  | N        | N     | Silent        | N
Task 2: shell wrapper preflight   | cargo install overwrites wrapper → events vanish  | N        | N     | Silent        | N
Task 2: shell wrapper pnpm fail   | cat build_log dumps npm registry auth tokens      | N        | N     | Silent        | Y (secrets)
Task 3: upstream_target() logging | URL with userinfo credentials logged verbatim     | N        | N     | Silent        | Y (secrets)
Task 3: resource_uri field        | Pre-signed S3 URI with Amz tokens logged raw      | N        | N     | Silent        | Y (secrets)
Task 4: JSON verification         | Dev-build verified, prod filter differs → drift   | N        | N     | Silent        | —
```

**CRITICAL GAPS** (RESCUED=N, TEST=N, USER SEES=Silent): rows 1, 3, 6, 7.

### NOT in Scope (Deferrable Without Blocking)

- Exact Axon color256 palette tuning — structure correct now, colors tunable after real-env review
- SmallVec field visitor — defer until profiling shows formatter in hot path
- `field_category()` refactor — acceptable to defer if ≤8 new semantic fields land in this PR
- `/ready` readiness probe — out of scope for this plan
- Formatter integration test (full subscriber stack) — just test pure helpers

### Summary

| Category | Critical | Important | Minor |
|----------|----------|-----------|-------|
| Architecture | 0 | 4 | 1 |
| Simplicity | 0 | 3 | 2 |
| Security | 3 | 2 | 1 |
| Performance | 1 | 2 | 1 |
| **Total** | **4** | **11** | **5** |

### Recommended Changes (Ordered by Impact)

1. **Add `sanitize_field_value()`** — strip C0 control chars from all field values before styling. Apply in `format_field_value()` and on message field. Must land before any Task 3 field additions.
2. **Replace `upstream_target()` with redacting variant** — strip userinfo from URL before logging; reject userinfo in config validation.
3. **Add `redact_resource_uri_for_logging()`** — strip query strings and fragments from `resource_uri` before any log site in Task 3.
4. **Remove shell wrapper from plan** — delete Task 2 shell wrapper steps; port preflight logging to `serve::run()` as a Rust `subsystem = "cli-preflight"` phase or drop entirely.
5. **Extract formatter into `crates/lab/src/tracing/`** — do this as part of Task 1; prerequisite for clean testing and prevents `main.rs` becoming a god file.
6. **Do not add `console` crate** — use `owo-colors` (already present) or extend existing `style(text, code)` helpers with named semantic methods.
7. **Verify `LogIngestLayer`** does not string-match on `subsystem`/`phase` values Task 2 renames.
8. **Change `BTreeMap<String, String>` to `BTreeMap<&'static str, String>`** — free allocation win, no behavior change.
9. **Remove redundant startup `tracing::info!` calls** in `run_http()` (~60 LOC, pure noise reduction).
10. **Place all new Task 2 events after `init_tracing()`** — audit placement of every new startup event.

```
Architecture: 4  |  Simplicity: 5  |  Security: 6  |  Performance: 4
Critical gaps: 4  |  TODOs proposed: 3
```

---

**Goal:** Make `lab serve` startup and request-path logging clearly observable in human mode, with Axon-mirroring terminal styling and plain/JSON-safe output behavior.

**Architecture:** Keep tracing event semantics as the source of truth and upgrade presentation separately. Normalize startup lifecycle events across CLI preflight, bootstrap, API server, web server, MCP server, and gateway/upstream manager, then route those same events through three output modes: TTY human, plain human, and JSON.

**Tech Stack:** Rust `tracing`, `tracing-subscriber`, `console::Style`, shell wrapper logging in `/home/jmagar/.cargo/bin/lab`, existing `lab serve` startup instrumentation.

---

## File map

- Modify: `crates/lab/src/main.rs`
  - Owns tracing subscriber setup and human-vs-json formatter selection.
  - Will become the single place where Axon-style `console::Style` rendering is defined.
- Modify: `crates/lab/src/cli/serve.rs`
  - Owns startup lifecycle instrumentation for bootstrap, API router/build/bind, web mount, MCP mount, and final ready state.
- Modify: `crates/lab/src/dispatch/upstream/pool.rs`
  - Owns gateway/upstream discovery and upstream request lifecycle events.
- Modify: `crates/lab/src/mcp/server.rs`
  - Owns MCP-facing dispatch/proxy event payload quality for tools/prompts/resources.
- Modify: `/home/jmagar/.cargo/bin/lab`
  - Owns CLI preflight / asset refresh observability before Rust startup begins.
- Optional docs check: `docs/OBSERVABILITY.md`
  - If event naming or required fields drift, update this doc in the same change.

---

### Task 1: Replace ad hoc human ANSI formatting with Axon-style `console::Style`

**Files:**
- Modify: `crates/lab/src/main.rs`
- Reference: `/home/jmagar/workspace/axon_rust/crates/core/logging.rs`
- Reference: `/home/jmagar/workspace/axon_rust/crates/core/ui.rs`

- [ ] **Step 1: Snapshot the Axon palette and formatter rules into the plan implementation notes**

Rules to mirror:
- `ERROR`: bold red
- `WARN`: bold yellow
- `INFO`: green
- `DEBUG` / `TRACE`: dim
- timestamp: dim
- keys / separators: dim
- important subject token: stronger emphasis
- Axon accents:
  - `primary = color256(211)`
  - `accent = color256(111)`
  - `subtle = color256(110)`

- [ ] **Step 2: Write a focused failing test or formatter assertion if the existing formatter is testable**

Target behavior:
- TTY human mode styles high-signal fields semantically.
- plain mode has no ANSI sequences.
- JSON mode bypasses the human formatter.

If there is no formatter test harness today, add the smallest targeted unit test around the style/render helper instead of broad subscriber integration tests.

- [ ] **Step 3: Replace raw ANSI string construction with `console::Style` helpers**

Implementation requirements in `crates/lab/src/main.rs`:
- define explicit style helpers for:
  - level labels
  - timestamp
  - subsystem/lane label
  - key names
  - separator punctuation
  - accented subject values
  - error values
  - warning/error `kind` values
- do not change JSON rendering path
- keep non-TTY output unstyled
- keep `NO_COLOR` handling

Concrete semantic mapping:
- `upstream`, `tool`, `prompt`, `resource_uri`, `route`: Axon accent blue
- `error`: red
- `kind` on warnings/errors: yellow
- low-signal metadata: dim or default

- [ ] **Step 4: Keep human layout stable while improving styling**

Do not redesign event semantics here. Preserve the existing field-aware layout shape, but make it visually hierarchical:
- lane/subsystem first
- operation/subject second
- elapsed third where present
- selected high-signal fields after that

- [ ] **Step 5: Build to catch formatter API mistakes early**

Run: `cargo build --all-features --manifest-path crates/lab/Cargo.toml`
Expected: success

- [ ] **Step 6: Commit formatter foundation**

```bash
git add crates/lab/src/main.rs
git commit -m "feat: adopt axon-style human log formatting"
```

---

### Task 2: Normalize startup lifecycle events across all `lab serve` subsystems

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `/home/jmagar/.cargo/bin/lab`
- Optional: `docs/OBSERVABILITY.md`

- [ ] **Step 1: Define one startup lifecycle vocabulary before editing logs**

Use these stable states only:
- `start`
- `finish`
- `ready`
- `disabled`
- `error`

Use these subsystem lanes consistently:
- `CLI`
- `CLI-PREFLIGHT`
- `STARTUP`
- `API-SERVER`
- `WEB-SERVER`
- `MCP-SERVER`
- `GATEWAY-CLIENT`

- [ ] **Step 2: Make CLI preflight explicitly observable in the wrapper**

In `/home/jmagar/.cargo/bin/lab`, ensure these are emitted when relevant:
- `CLI-PREFLIGHT web.assets.refresh.start`
- `CLI-PREFLIGHT web.assets.refresh.finish`
- `CLI-PREFLIGHT web.assets.refresh.error`

Requirements:
- do not dump successful build noise by default
- preserve failure output on error
- preserve plain vs colored behavior based on TTY and `NO_COLOR`

- [ ] **Step 3: Make Rust bootstrap phases explicit in `serve.rs`**

Ensure each of the following has a clear lifecycle line:
- bootstrap start
- config/auth summary
- upstream OAuth runtime enabled/disabled
- gateway discovery start/finish
- gateway manager ready
- API router build start/finish
- listener bind start
- API server ready
- web server mount start and ready/disabled
- MCP server mount start and ready/disabled
- final global ready

- [ ] **Step 4: Remove or consolidate redundant aggregate startup lines**

If two lines communicate the same state, keep the more specific one. Prefer subsystem-specific state over vague global summaries.

- [ ] **Step 5: Verify startup event coverage by running the binary directly**

Run: `target/debug/lab serve --port 9876`
Expected:
- explicit subsystem lifecycle lines appear
- API/web/MCP/gateway states are distinguishable
- failure/disabled paths remain intelligible

- [ ] **Step 6: Commit startup taxonomy pass**

```bash
git add crates/lab/src/cli/serve.rs /home/jmagar/.cargo/bin/lab docs/OBSERVABILITY.md
git commit -m "feat: improve lab serve startup observability"
```

---

### Task 3: Make upstream and MCP high-signal fields glanceable

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/pool.rs`
- Modify: `crates/lab/src/mcp/server.rs`

- [ ] **Step 1: Audit current event payloads for high-signal identifiers**

For each event family, confirm the identifying field is always present:
- tool calls: `tool`
- prompt fetches: `prompt`
- resource reads: `resource_uri`
- upstream request lifecycle: `upstream`
- route decisions: `route`
- failures: `error` and `kind` where applicable

- [ ] **Step 2: Add missing fields before touching appearance**

Do not rely on formatter tricks to compensate for missing event data. If a warning or info line still lacks the real subject, add the field at the event site.

- [ ] **Step 3: Make sure the formatter classifies these fields semantically**

The formatter in `main.rs` must recognize and accent these keys:
- `upstream`
- `tool`
- `prompt`
- `resource_uri`
- `route`
- `error`
- `kind`

- [ ] **Step 4: Verify against the known bad case**

Use a real startup/run where unsupported upstream prompt/resource listing occurs. The resulting warnings should make:
- upstream name visibly accented
- error text visibly non-default and non-white
- lane and event body easy to scan

- [ ] **Step 5: Commit high-signal field pass**

```bash
git add crates/lab/src/dispatch/upstream/pool.rs crates/lab/src/mcp/server.rs crates/lab/src/main.rs
git commit -m "feat: highlight upstream and MCP subject fields"
```

---

### Task 4: Verify all three output modes in the real environment

**Files:**
- No new code required
- Verification only

- [ ] **Step 1: Verify all-features build stays green**

Run: `cargo build --all-features --manifest-path crates/lab/Cargo.toml`
Expected: success

- [ ] **Step 2: Verify TTY human output**

Run interactively:
- `lab serve --port 9876`

Expected:
- Axon-style colors are visible
- startup subsystem states are obvious
- high-signal fields are accented
- warnings/errors are glanceable

- [ ] **Step 3: Verify plain piped output has no ANSI bleed**

Run:
```bash
lab serve --port 9876 2>&1 | sed -n '1,80p'
```

Expected:
- no raw escape sequences
- same event content remains readable and grepable

- [ ] **Step 4: Verify JSON path is untouched**

Run:
```bash
LAB_LOG_FORMAT=json lab serve --port 9876 2>&1 | sed -n '1,20p'
```

Expected:
- newline-delimited JSON
- no human formatter styling artifacts

- [ ] **Step 5: Verify wrapper preflight path through the installed command**

Run:
- `lab serve --port 9877`

Expected:
- wrapper preflight logs appear only when preflight work happens
- successful preflight does not dump noisy build output
- failures still surface underlying command output

- [ ] **Step 6: Record any environment-specific caveats**

If a port conflict or stale asset condition affects verification, note it in the final handoff so runtime behavior is distinguished from local-machine interference.

---

### Task 5: Final review and handoff

**Files:**
- Optional: `docs/OBSERVABILITY.md`
- Final handoff notes

- [ ] **Step 1: Sanity-check observability docs for drift**

If event naming, required fields, or startup expectations changed in a material way, update `docs/OBSERVABILITY.md` in the same change.

- [ ] **Step 2: Summarize final operator-visible outcomes**

Final handoff should explicitly state:
- which startup subsystems are now observable
- which fields are semantically colored in TTY mode
- how plain mode behaves
- how JSON mode behaves
- any known local-environment caveats found during verification

- [ ] **Step 3: Final commit**

```bash
git add crates/lab/src/main.rs crates/lab/src/cli/serve.rs crates/lab/src/dispatch/upstream/pool.rs crates/lab/src/mcp/server.rs /home/jmagar/.cargo/bin/lab docs/OBSERVABILITY.md
git commit -m "feat: upgrade lab serve observability and human log UX"
```
