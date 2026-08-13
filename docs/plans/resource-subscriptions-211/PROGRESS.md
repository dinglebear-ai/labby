# Progress — Legacy resource subscription passthrough

- **Issue:** [#211](https://github.com/dinglebear-ai/labby/issues/211) · **Epic:** `lab-n27j2`
- **Branch:** `feat/resource-subscriptions-211` · **Integrated base:** `origin/main` at `313e58969`
- **Status:** Researched, reviewed, **rescoped, and P0 implemented**; handler build deferred.
- **Last updated:** 2026-08-13

This is the handoff surface. Anyone picking this up should read
[FINDINGS.md](FINDINGS.md) and this page and know exactly where things stand.

---

## Current shape

| Priority | Work | Bead | Status |
|---|---|---|---|
| **P0** | Version-conditional capability advertisement + two comment fixes | `lab-n27j2.4` | ☑ done |
| **S1** | Missing auth-scope gate on subscription acceptance (modern path, live on `main`) | to create | ☐ |
| **S2** | `ui://` route-scope bypass | `lab-1415y` | ☐ open |
| **S3** | `Lagged` warn fields; tracing in `notify_resource_update_peers` | to create | ☐ |
| **S4** | `prune_closed_peers` call site independent of `listen()` | to create | ☐ |
| **S5** | P-1 predicate short-circuit; P-2 filter-under-read-lock | to create | ☐ |
| **S6** | `docs/services/GATEWAY.md` stale legacy-`initialize` claim | to create | ☐ |
| **A–E** | Handler build, pool API (B0), bridge | `lab-n27j2.1/.2/.3` | ⊘ **Blocked on demand** |

Legend: ☐ not started · ◐ in progress · ☑ done · ⊘ blocked

**Blocking condition for A–E:** identify a real client that is (a) pre-`2026-07-28`,
(b) connected over stdio, and (c) using `resources/subscribe` rather than
`subscriptions/listen`. If none surfaces, close #211 as resolved by P0.

---

## Open items — all resolved

### O-1 — How does a request handler identify its session? **RESOLVED**

`context.protocol_version()` is public rmcp API (`service.rs:1221-1229`); rmcp
reconstructs `peer_info` in stateless mode specifically so it works inside
handlers (`tower.rs:1959-1963`).

Two corrections to how this was originally framed:

- **The question was largely moot.** rmcp already gates the handler on session
  era (`handler/server.rs:185-201`), so the hand-rolled version check was never
  needed.
- **The risk polarity was backwards.** The plan feared a *shared*
  `LabMcpServer` leaking subscriptions across sessions. The real hazard is the
  opposite: on HTTP the instance is **not** shared — a fresh one per POST
  (`tower.rs:1947`) — so every request mints a new identity. On stdio there is
  exactly one for the process, where a per-instance cell is correct.

### O-2 — Does the bridge↔daemon session negotiate `2026-07-28`? **RESOLVED — and the version was the smaller problem**

Yes: `live_gateway.rs:434-437` uses
`ClientLifecycleMode::Discover{V_2026_07_28}`. But the transport is
`StreamableHttpClientWorker` — stateless HTTP — so the daemon has **no push
channel to the bridge at all**. Neither remedy the plan proposed addressed that.

Compounding: `Peer::listen`'s documentation states that notifications routed to
the returned `Subscription` are *not also* delivered through `ClientHandler`
callbacks (`service/client.rs:1058-1059`), so the planned
`BridgeClientHandler::on_resource_updated` is dead code in both branches.

### O-3 — Are the conformance suites nextest-filtered or scenario-driven? **RESOLVED — both, and neither helps**

- `mcp-regressions` (`.github/workflows/ci.yml:548-588`) is a list of explicit
  `cargo test <filter>` invocations. **New tests are not auto-discovered** —
  tagging does nothing; you must add a filter line.
- `mcp-conformance` drives the pinned rmcp fixture at
  `MCP_SPEC_VERSION=2026-07-28` (`scripts/ci/mcp-conformance.sh:37-39`) and will
  never send a legacy `resources/subscribe`.

---

## Decisions — revised after review

| ID | Decision | Status |
|---|---|---|
| D1 | Local `lab://` URIs rejected | holds |
| D2 | `-32002`; out-of-scope indistinguishable from not-found; recovery contract in `error.data` | holds — and `-32002` confirmed era-correct per SEP-2164 (C-17) |
| D3 | Legacy peers pruned on timeout only when transport is closed | **amended** — needs a consecutive-failure bound (k=3), else a wedged peer taxes every event by the full 5 s timeout forever |
| D4 | Strict rejection during the reconnect window, with `retry_later` | holds |
| D5 | Reject 2026-07-28 sessions from legacy subscribe | **superseded** — rmcp enforces it; gate G-2 deleted |
| D6 | Reuse `RegisteredPeer`/`LegacyPeer` with an interior-mutable set | holds — plus a lock-ordering invariant (TYPES §2.2b) |
| D7 | `wants_*_list_changed → false` for legacy subscribers | holds — but it is a *semantic* gate only; the contract recompute still runs (S5) |
| D8 | Duplicate subscribe / unknown unsubscribe idempotent | holds — plus C-8b: unsubscribe must never register |
| D9 | Empty URI set removes the entry | holds |
| D10 | Upstream-side legacy subscribe is a non-goal | holds |
| D11 | Do not copy the modern path's `ui://` scope bypass | holds — and review argues `lab-1415y` should land *alongside* the B0 extraction, not be deferred indefinitely |
| D12 | Repo style constraints | holds |
| **D13** | **New:** G-0 transport gate is mandatory before any handler | added |
| **D14** | **New:** per-session subscription cap (G-5), keyed on session identity | added |
| **D15** | **New:** bridge opens one daemon-side `listen()` at connect; never re-listen per subscribe | added |

---

## Acceptance criteria — P0

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Legacy `initialize` does not advertise `resources.subscribe` | ☑ | `legacy_initialize_withholds_resource_subscribe_capability` PASS; plus the pre-existing end-to-end `cli::serve::tests::http_mcp_adapts_legacy_initialize_lifecycle` PASS |
| 2 | Modern `discover` still does | ☑ | `get_info_does_not_withhold_capabilities_from_modern_sessions` PASS. Partial by construction — the fixture has `gateway_manager: None`, so it locks in that `get_info` performs no withholding rather than that the flag is present. Criterion 3 is the real guard |
| 3 | Modern `subscriptions/listen` with `resource_subscriptions` still delivers end to end | ☑ | `stateless_subscription_receives_catalog_notifications` PASS — a live modern subscription; this is what would have caught a global clear |
| 4 | Upstream negotiation unaffected (`pool/notifications.rs:270`) | ☑ | Whole `labby-gateway` suite passes within the 2436 |
| 5 | Two false session-lifetime comments corrected | ☑ | `server.rs` relay-counter doc; `serve.rs` `build_mcp_service` doc |
| 6 | `docs/surfaces/MCP.md` states the boundary and why | ☑ | New "Resource Subscriptions" section |
| 7 | `just lint`, `just test`, `just docs-check` green | ☑ | See verification log. One caveat recorded there: `clippy --all-targets` surfaces two pre-existing failures outside the repo's gate |

> **Original criterion 10 is void.** "Delivery verified on stdio and
> streamable-HTTP" asserts what F-0 proves impossible. Any future handler build
> must replace it with a negative assertion: HTTP subscribe rejected, registry
> length unchanged.

---

## Verification log

Evidence, not intentions.

| Date | Check | Result |
|---|---|---|
| 2026-08-05 | `git grep -nE "fn (subscribe\|unsubscribe)" origin/main -- crates/labby/src/mcp/server.rs` | No match — gap confirmed |
| 2026-08-05 | `git grep -n "enable_resources_subscribe" origin/main` | `server.rs:415` — capability already advertised |
| 2026-08-05 | `serve.rs:1720,1738` | `NeverSessionManager`, `legacy_session_mode(false)`, `json_response(true)` — F-0 confirmed |
| 2026-08-05 | `rmcp tower.rs:1512-1521` | `(false,false) ⇒ "POST"` — GET returns 405, no SSE stream |
| 2026-08-05 | `rmcp tower.rs:1947,1968` | Fresh `LabMcpServer` + `OneshotTransport` per POST |
| 2026-08-05 | `rmcp tower.rs:2098` | Headerless POST defaults to `V_2025_03_26` ⇒ classified legacy |
| 2026-08-05 | `rmcp handler/server.rs:185-201`, `:146-149`, `service.rs:196-202` | Both subscribe *and* listen version-gated by the SDK |
| 2026-08-05 | `rmcp model.rs:2013-2018` | `supported_by` reads the same `resources.subscribe` flag ⇒ cannot drop it globally |
| 2026-08-05 | `rmcp service/client.rs:1058-1059`, `:322-324`; `service/server.rs:139-144` | `ClientHandler` callbacks bypassed; streams not resumable; filter immutable |
| 2026-08-05 | `grep -rn prune_closed_peers crates/` | One production call site — `server.rs:538`, inside `listen()` |
| 2026-08-05 | `catalog_notifications.rs:342-350` | `visible_contract().await` precedes the predicate — confirmed |
| 2026-08-05 | `grep "fn upstream_owning_resource\|fn catalog_lists_resource"` | Zero matches — both helpers fictional |
| 2026-08-05 | `bridge.rs:69` | `pub struct BridgeClientHandler;` — zero-field unit struct |
| 2026-08-05 | `cargo nextest run --workspace --all-features` | **2436 tests run: 2436 passed**, 7 skipped |
| 2026-08-05 | `cargo clippy --workspace --all-features -- -D warnings` (the command `just lint` runs) | clean |
| 2026-08-05 | `cargo fmt --all -- --check` | clean |
| 2026-08-05 | `labby docs check` | `checked 17 docs artifacts: fresh` |
| 2026-08-05 | `cargo check -p labby --no-default-features --features gateway` | builds |
| 2026-08-05 | `cargo clippy --workspace --all-features --all-targets` | **2 errors — PRE-EXISTING, not from P0.** `clippy::panic` at `crates/labby-runtime/tests/agent_error_schema.rs:88,96`. That crate is byte-identical to `origin/main` (`git diff origin/main -- crates/labby-runtime/` is empty), and `just lint` does **not** pass `--all-targets`, so the repo's gate never sees it. Issue #211's text asks for the stricter form, under which `main` is already red. Worth its own bead; deliberately not folded into P0 |
| 2026-08-13 | Rebase onto `origin/main` (`313e58969`) | clean; P0 implementation and tests retained |
| 2026-08-13 | `just lint` | clean |
| 2026-08-13 | `just test` | **2732 passed, 3 failed, 7 skipped.** The three `xtask::proxy_verify_cli` failures were shared-target interference: `CARGO_BIN_EXE_xtask` disappeared while other worktrees built. Immediate rerun in an isolated `CARGO_TARGET_DIR` passed 3/3. |
| 2026-08-13 | `CARGO_TARGET_DIR=/tmp/labby-resource-subscriptions-target cargo nextest run -p xtask --test proxy_verify_cli` | **3 passed**; confirms the full-suite failures were environmental rather than branch regressions |
| 2026-08-13 | focused P0 tests, gateway feature slice, `just docs-check` | both P0 tests passed; slice built; 17 generated docs artifacts fresh |

---

## Rebase watch

`audit/mcp-2026-07-28-capabilities` — **risk overstated in the first draft.** It
touches neither `LegacyPeer`, the prune logic, nor subscribe/unsubscribe; it is
stale relative to `main` rather than competing. Real risk is textual merge noise
in `server.rs`/`bridge.rs` from unrelated refactors.

`origin/main` advanced past `132448802` during this review. Re-verify citations
before implementing.

---

## Session log

| Date | Session | What happened |
|---|---|---|
| 2026-08-05 | Planning | Researched #211; found the issue's plan stale. Created epic `lab-n27j2` + children, bug `lab-1415y`. Wrote the artifact set. Created worktree. No code. |
| 2026-08-05 | Research (9 agents) | Found **F-0**: the feature cannot work on HTTP, and HTTP is the likely path. Also: rmcp already gates modern sessions; `prune_closed_peers` never runs in the target deployment; two planned pool helpers don't exist; a missing auth gate; a contract gate-ordering leak. Five agent claims refuted on verification (FINDINGS §7). |
| 2026-08-05 | Eng review (4 agents) | Converged on **do not build as scoped**. Established the version-conditional P0 fix, that Phase D is dead in both branches, that gate G-4 cannot currently fire, and that most performance findings evaporate at n=1. Rewrote all artifacts. |
| 2026-08-13 | P0 implementation | Withheld `resources.subscribe` only from legacy `initialize`, preserved modern `discover`/`subscriptions/listen`, corrected HTTP instance-lifetime comments, added regression tests and surface documentation, and rebased onto current `origin/main`. |
