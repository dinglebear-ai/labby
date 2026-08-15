# PROGRESS — issue #210

**Update in the same commit as the work it describes.** Beads (`bd show lab-41e7m`) are the
source of truth for status; this is the human-readable roll-up plus the record of audits,
decisions, and findings.

| | |
|---|---|
| Epic | `lab-41e7m` |
| Issue | [#210](https://github.com/dinglebear-ai/labby/issues/210) |
| Branch | `feat/mcp-output-schema-210` |
| Worktree | `.worktrees/feat-mcp-output-schema-210` |
| Base | `origin/main` @ `43d51ec73` (merged 2026-08-11; original plan base `132448802`) |
| Phase | **All epic beads implemented** (`.1`–`.7`, 6 commits on this branch). Remaining: FU-1..FU-8 follow-ups + `lab-kooxf` (OpenAPI response schemas), and the AC-16 caveat in the eventual PR description + issue comment |
| Last updated | 2026-08-11 |

---

## 1. Status

| Bead | Title | Status | Blocked by |
|---|---|---|---|
| `lab-41e7m` | Epic | open | — |
| `lab-41e7m.1` | Envelope schema + registry builder + FR-7 | **implemented** (this commit) | — |
| `lab-41e7m.2` | Lock Code Mode unwrap + truncation + success-path proxy fidelity | **implemented** (this commit) | — |
| `lab-41e7m.3` | Catalog output-shape coverage | **implemented** (this commit) | ~~`lab-41e7m.7`~~ done |
| `lab-41e7m.4` | Docs, generated artifacts, conformance | **implemented** (this commit) | ~~.1, .2, .3~~ done |
| `lab-41e7m.5` | **FR-2a** — authorization-gate consolidation (moved OUT of `.1`) | **implemented** (this commit) | — |
| `lab-41e7m.6` | FR-9a — sanitize upstream metadata on `tools/list` (HIGH) | **implemented** (this commit) | — |
| `lab-41e7m.7` | FR-9b — bound `$ref` expansion (HIGH, blocks `.3`) | **implemented** (this commit) | — |
| **to file** | FU-1..FU-8 follow-ups (SPEC §6.2) | — | — |

```bash
bd show lab-41e7m
bd ready
bd update lab-41e7m.1 --claim
```

## 2. Requirements

Defined in [SPEC.md](SPEC.md) §3–§4. Acceptance criteria live in SPEC §6 — **single source; this
file tracks status only, it does not restate them** (an earlier draft restated them in four
places, which is the exact drift this issue exists to eliminate).

| Req | Summary | Bead | Status |
|---|---|---|---|
| FR-1 | Envelope `outputSchema` on builtins (Raw mode only) | .1 | ☑ 2026-08-11 |
| FR-2 | One builder — extend `PermanentToolRegistry` | .1 | ☑ 2026-08-11 |
| FR-2a | Consolidate duplicated authorization gates | `lab-41e7m.5` | ☑ 2026-08-11 |
| FR-3 | Audit before attachment — shape **and** presence | .1 | ☑ §3 below |
| FR-4 | Unwrap precedence documented + tested | .2 | ☑ 2026-08-11 |
| FR-5 | Structure survives truncation — **both** markers | .2 | ☑ 2026-08-11 |
| FR-6 | Success-path proxy fidelity | .2 | ☑ 2026-08-11 |
| FR-7 | Error trace ↔ trace schema consistency | .1 | ☑ 2026-08-11 |
| FR-8 | Catalog coverage | .3 | ☑ 2026-08-11 |
| FR-9a | Sanitize upstream metadata on `tools/list` | `lab-41e7m.6` | ☑ 2026-08-11 |
| FR-9b | Bound `$ref` expansion | `lab-41e7m.7` | ☑ 2026-08-11 |

**Acceptance criteria:** AC-1, AC-2, AC-2a, AC-3, AC-15 ☑ (tests in
`handlers_tools/tests.rs` §"Issue #210" and `permanent_tools.rs::tests`); AC-16 lands with
`.4` plus the PR description and issue comment. Others track their beads. See SPEC §6.

## 3. Audit results

Required by FR-3 before any schema is attached. **Two axes** — shape *and* presence.

Audited 2026-08-11 at merge `e0c6bf7ec`. Every registry service's success path funnels
through one chokepoint — the generic dispatch at `call_tool.rs` → `format_dispatch_result`
(`result_format.rs:124-127`), which always sets the envelope as `structuredContent` — so
the builtin rows share one verification site. Stub services can only return
`unknown_action` errors (exempt); attaching the schema to them is harmless and keeps the
loop uniform. Elicitation `input_required` is a distinct result variant, outside the
contract (C2.3).

| Tool | Class | Success shape | `structuredContent` always set? | Verified at | Schema | Notes |
|---|---|---|---|---|---|---|
| `gateway` | builtin | envelope | yes | `result_format.rs:124-127` via `call_tool.rs` generic path | envelope | |
| `doctor` | builtin | envelope | yes | same chokepoint | envelope | |
| `setup` | builtin | envelope | yes | same chokepoint | envelope | |
| `server_logs` | builtin | envelope | yes | same chokepoint | envelope | meta unchanged, still audience-gated |
| `snippets` | builtin | envelope | yes | `services/snippets.rs:77` → `format_dispatch_result` | envelope | |
| `fs` | builtin | envelope | yes | generic path (`services/fs.rs` builds no `CallToolResult`) | envelope | |
| `lab_admin` | builtin | envelope | yes | same chokepoint | envelope | |
| `mcp_app` | synthetic | `{"kind":"mcp_app_control", …}` | yes (`call_tool.rs:410-411`) | read | **none** | shape axis fails — accurate-or-none rule |
| `add_server` | synthetic | envelope (`open`/`test`/`create`) | yes | `call_tool.rs` → `format_dispatch_result` | envelope | |
| `gateway_status` | synthetic | envelope (`open`/`refresh`) | yes | `call_tool.rs` → `format_dispatch_result` | envelope | |

**OpenAPI provider output schemas (FR-8 / plan §5.3):** audited 2026-08-11.
`labby-openapi/src/convert.rs` extracts only dispatch metadata (operationId, HTTP method,
path template) — no `responses` parsing exists in the crate, and OpenAPI operations dispatch
through the reserved `openapi` local provider without per-operation `ToolDescriptor`s, so
nothing fabricates output types today. Deriving output schemas from spec `responses` is
deferred to its own bead (`lab-kooxf`, filed 2026-08-11) per the plan's
truthfulness rule: a spec-derived schema is the one place a WRONG schema can be introduced.

## 4. Verification log

| Date | Command | Result | Notes |
|---|---|---|---|
| 2026-08-11 | `cargo nextest run --workspace --all-features` | **2557 passed, 0 failed** | includes 6 new #210 tests |
| 2026-08-11 | `just lint` (clippy `-D warnings` + fmt + drift checks) | clean | CI's clippy runs **without** `--all-targets`; the plan's `--all-targets` variant surfaces two pre-existing `clippy::panic` warnings in `labby-runtime/tests/agent_error_schema.rs` that also exist on main — out of scope |
| 2026-08-11 | `just docs-generate && just docs-check` | fresh (no-op) | confirmed F22/NFR-8: artifacts render from the action catalog |

## 5. Decision log

| # | Decision | Rationale | Where |
|---|---|---|---|
| D1 | Success-only `outputSchema` | **Rationale corrected:** no spec text exempts `isError`; it is converged convention, and upstream rejected widening schemas to cover errors. Decisive repo-local reason: `3e5ab3df` made the error envelope a schema-locked contract that *rewraps* success-shaped content | SPEC §5.1, CONTRACT §C3.2 |
| D2 | One shared generic schema, `service: string` | Preserves shared `Arc`; `const` adds nothing the envelope already carries | SPEC §5.2 |
| D3 | **Add constructors to `permanent_tools.rs`; extend the existing drift test** | The drift test genuinely already exists and is stronger. The registry is a 1-entry *identity* registry, not yet a descriptor factory — the pattern is precedented once, by `code_mode_descriptor`. Ship either this or a descriptor module owning `code_mode_descriptor`, not both; rewrite the module doc in the same commit | SPEC §5.6, RESEARCH §2 |
| D4 | No protocol-version gating | rmcp serializes regardless of version; old clients ignore unknown fields | SPEC §5.3 |
| D5 | Unwrap: lock, don't change | Byte-identical since 2026-05-31 across three refactors | SPEC §5.4 |
| D6 | Snippets keep `output_schema: None` | A snippet returns an arbitrary JS value | SPEC FR-8 |
| D7 | FR-7 fix = add `logs_count: 0` | **Reason corrected:** internal consistency for trace consumers, NOT conformance — the trace is `isError` and therefore exempt, so the original framing contradicted D1 | SPEC FR-7 |
| D8 | No per-action output schemas | **Mitigation withdrawn:** the `schema` action returns `ActionSpec.returns`, explicitly "not a runtime contract." Recorded as a limitation; `ActionSpec.returns` is the seam if it is ever built | SPEC NG-1, MODELS §4.1 |
| D9 | No `DispatchEnvelope` struct | Envelope is a `json!` literal | MODELS §2.1 |
| D10 | **Follow the repo's plain-JSON drift-test pattern** | `docs/contracts/schemas/` + a test reading the file as data is an established, code-enforced convention. No validator dependency | SPEC NFR-10, SCHEMAS |
| D11 | Ship FR-1 scoped and labelled Raw-mode-only | Builtins are suppressed under Code Mode; exposing them via the catalog is materially larger (FU-1). Must not ship as "output shapes now reach agents" | SPEC §2.1, AC-16 |
| D12 | Schemas reduced 4 → 2 | Error envelope duplicated a published schema; catalog descriptor duplicated a Rust struct with no enforcing test | SCHEMAS |
| D13 | **`additionalProperties: true`** — decided, not a tiebreak | `false` would break all 7 builtins' schemas client-side on any future envelope field *and* move the contract hash; this envelope family demonstrably grows. Internal detectability preserved by the "exactly four keys" test | SPEC §5.2, CONTRACT §C3.5 |
| D14 | **FR-2a moves to its own bead** | 8 call sites, 6 outside `.1`; it changes authorization, not descriptor shape. The consolidated gate MUST stay audience-free and MUST be a free function, not reached via `self.peer_contract()` | SPEC FR-2a |
| D15 | **FR-9b blocks `.3`; memoization struck** | The render cache rebuilds outside its lock, so a hostile upstream gets an N× multiplier on an exponential expansion; `(ref, root)` memoization is semantically wrong and bounds no output | SPEC FR-9b |
| D16 | **FR-9a must be keyword-scoped** | `sanitize_schema` rewrites `enum`/`const`/`pattern` too, which would make Labby advertise a schema its own byte-identical results violate | SPEC FR-9a |

## 6. Findings

Discovered by reading the tree, not the issue. F1–F7 from planning; F8+ from the research pass.

| # | Finding | Impact | Where |
|---|---|---|---|
| F1 | Most of #210 is already implemented | Scope shrank from "build" to "close gaps + lock" | SPEC §1.1 |
| F2 | Independent descriptor builders | High-probability bug class | SPEC G2 |
| F3 | ~~`LazyLock` vs per-call alloc proves drift~~ | **SUPERSEDED** — same value; an allocation difference, not drift evidence | RESEARCH §3 |
| F4 | `codemode` error trace omits required `logs_count` | Real, but exempt — internal inconsistency only | SPEC FR-7 |
| F5 | `json_schema_to_type(None)` yields `unknown`, not `any` | Absent schema handled truthfully | TYPES §3.2 |
| F6 | No `lab.help` global tool exists | Prevented scoping work to a non-existent tool | plan §2 |
| F7 | `worktree-setup` scripts SIGPIPE on many-worktree repos | Blocked worktree creation; worked around | §8 |
| **F8** | **`outputSchema` is invisible under Code Mode** — builtins suppressed except `server_logs` | **Scope inversion; largest finding** | SPEC §2.1 |
| **F9** | **`PermanentToolRegistry` already is the proposed seam** | Avoided a parallel module | RESEARCH §2a |
| **F10** | **A stronger drift test already ships** (`tests.rs:1671`), gap is coverage not existence | Avoided writing a weaker test | RESEARCH §2b |
| **F11** | **Four duplicated sites, not two** — `catalog.rs`/`peer_contract.rs` gates duplicate authorization logic | Widened FR-2 → FR-2a | RESEARCH §3 |
| **F12** | **No spec text exempts `isError`** — convention only; TS SDK client throws `-32602` today | D1 rationale rewritten | RESEARCH §4 |
| **F13** | **Python SDK hard-errors** on declared-schema-without-`structuredContent`; already broke Claude Code's Bash tool | FR-3 gained a second axis | RESEARCH §5 |
| **F14** | **Upstream metadata unsanitized on `tools/list`** | HIGH security; contract would have codified it | SPEC FR-9a |
| **F15** | **`$ref` expansion is O(B^depth)** — depth-capped, not memoized | HIGH security (DoS) | SPEC FR-9b |
| **F16** | FR-6 "byte-identical" is false for errors — `enrich_completed_tool_error_result` rewraps deliberately | FR-6 scoped to success path | RESEARCH §7 |
| **F17** | **Two truncation markers**; the plan documented the non-default one | FR-5 covers both | RESEARCH §8 |
| **F18** | NG-1's mitigation is hollow — `ActionSpec.returns` is informational | Downgraded to a limitation | RESEARCH §9 |
| **F19** | `output_schema` is inside `descriptor_contract_hash` | One-sided drift breaks change detection; one-time fanout on upgrade | SPEC §2.3 |
| **F20** | Repo publishes contract schemas with code-enforced drift tests | Answered D10 with an existing pattern | RESEARCH §12 |
| **F21** | Example test fixtures in the first draft were invented | Replaced with the real `test_server`/`serve_directly` pattern | plan §3.6 |
| **F22** | `docs-check` will not break — generated artifacts render from the action catalog, not `Tool` | `.1` can land with docs green | SPEC NFR-8 |
| **F23** | `codemode.search` omits `dts`/`output_schema`; `describe` fails open silently | Agent may get type-less results that look complete | TYPES §3.2 |

## 6a. Engineering review — 2026-08-05 (post-revision)

Four agents re-reviewed the **revised** package, each given its own prior findings to verify.

| Agent | Verdict on the revision |
|---|---|
| architecture-strategist | Prior HIGHs substantively fixed; **do not start `.1` as scoped** — split FR-2a, fix the test payload (F24), resolve `additionalProperties` (F30) |
| code-simplicity-reviewer | Consolidation genuinely simplified; "extend the registry" framing overstated (F33); `code-mode-trace` publication is scope creep (→ FU-8) |
| security-sentinel | Fix *directions* sound, but the FR-9a fix **as specified introduces a new HIGH** (F26) |
| performance-oracle | Still performance-safe; strike memoization (F27); fanout claim false (F25); test not hermetic (F29) |

Net: 3 HIGH and 6 MEDIUM issues found in the revision itself — including two cases where a
proposed *fix* would have caused the failure class it was meant to prevent (F26) or written a
false mechanism into permanent docs (F25). All are folded in above.

## 7. Research pass — 2026-08-05

Ten domain-matched agents. Full evidence in [RESEARCH.md](RESEARCH.md).

| Agent | Verdict |
|---|---|
| architecture-strategist | ⚠ Two proposals reinvented existing seams |
| code-simplicity-reviewer | ⚠ Package oversized ~10:1 vs the diff |
| best-practices-researcher | ⚠ Spec citation wrong; real client hazards |
| framework-docs-researcher | ✓ All 9 rmcp API claims correct |
| learnings-researcher | ✓ Drift class documented as chronic (`lab-3qn`) |
| security-sentinel | ⚠ Two HIGH defects |
| performance-oracle | ✓ Plan perf-safe; 3 adjacent issues |
| pattern-recognition-specialist | ⚠ Four sites, not two |
| agent-native-reviewer | ⚠ Scope inversion (F8) |
| git-history-analyzer | ✓ FR-6 new; FR-7 dated; unwrap stable |

**Conflict resolved.** Simplicity argued the extraction over-solves (a test alone catches drift);
institutional memory (`lab-3qn`) records this drift class as chronic across five subsystems with
consolidation as the desired end-state. Architecture and pattern analysis resolved it: **both the
builder and the drift test already exist** — so the minimal change and the consolidating change
are the same change. Recorded as D3.

**Package proportionality.** The simplicity reviewer judged ~800 of ~2,200 lines removable.
Applied: schemas 4 → 2; acceptance criteria consolidated into SPEC §6 with other docs linking.
MODELS/TYPES retained — explicitly requested deliverables — but corrected and trimmed.

## 8. Known issues / follow-ups

- **FU-1 … FU-7** — SPEC §6.2 (Code Mode catalog exposure, three perf fixes, local-provider
  discoverability, duplicated upstream-merge loops, `ActionSpec.returns` → schema).
- **F7 — worktree tooling bug (external, not labby).** `worktree-new.sh:60`,
  `worktree-sync.sh:124`/`:242` pipe `git worktree list` into an `awk` that exits early; under
  `set -o pipefail` a many-worktree repo aborts with a silent exit 141. Fix:
  `awk '/^worktree /{if(!s){print $2; s=1}}'`. Filed as a separate task; this worktree was
  created with patched copies.
