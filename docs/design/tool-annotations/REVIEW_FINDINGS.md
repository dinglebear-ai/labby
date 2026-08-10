# Review Findings — Tool Annotations

Consolidated, reconciled output of two review rounds against the design package.
Every claim here was verified against the branch source by at least one agent;
conflicts between agents are resolved explicitly rather than averaged.

- **Research round** (7 agents): architecture, security, MCP best-practice, rmcp
  API, simplicity, patterns, agent-native. `learnings-researcher` skipped — repo
  has no `.lavra/memory` store.
- **Engineering round** (4 agents): architecture (failure modes), security
  (verdict), simplicity (scope), performance.

Verdict: **GO-WITH-CONDITIONS**. The one gating unknown (F9) was resolved on
2026-08-05 — accepted, Option A. All remaining conditions are in § 6.

---

## 1. Corrections to the design package (all confirmed)

| # | Document | Was | Is |
|---|---|---|---|
| C1 | SPEC § 5 `add_server` | "delegates to `gateway.test`/`gateway.add`, **both destructive-gated**" | **False.** Both are `destructive: false` (`labby-gateway/src/gateway/catalog.rs:527-529`, `:706-708`). The `destructiveHint: true` value survives on other merits (persists config, spawns a subprocess); the citation must go. |
| C2 | SPEC § 5 `fs` | "0 / 2 — `fs.list`, `fs.preview`" | MCP registers only `fs.list` (`mcp/services/fs.rs`); `fs.preview` is excluded (`registry.rs:494-503`) and rejected at the MCP surface (`dispatch/fs/dispatch.rs:83-87`, `http_only`). |
| C3 | SPEC § 10.1 / Risk R4 | "one-time `tools/list_changed` for every connected peer on upgrade" | **Cannot happen.** `server.rs:531-546` seeds `RegisteredPeer.last_contract` at registration; `catalog_notifications.rs:342-350` notifies only on a diff against that seed. Annotations derive from `&'static` data, so they change only across a binary change → process restart → all sessions destroyed → reconnecting peers seed from post-change descriptors. Delete the claim; do not put it in release notes. |
| C4 | SPEC § 9 acceptance #6 | "hash changes exactly once vs baseline" | Only assertable against a magic constant. Replace with two self-describing assertions (§ 4 below). |
| C5 | CONTRACT C3 | per-action truth "is published in `lab://<service>/actions` and `lab://catalog`" | True for the **seven service tools only**. `codemode`, `codemode_ui`, `mcp_app`, `add_server`, `gateway_status` are built in `handlers_tools.rs` and appear in no catalog, and have no `help` action. For those five the coarse hint is all an agent gets. |
| C6 | CONTRACT C3 fallback | cites MCP resources | Resources are an optional client capability. The always-reachable path is `tools/call` with `{"action":"help"}`, which already returns per-action `destructive` (`dispatch/helpers.rs:201`). Cite `help` first, resources second. |
| C7 | TYPES § T5 | builder line numbers `:122/:128/:134/:140` | Drift +3 — `.read_only` `:125`, `.destructive` `:131`, `.idempotent` `:137`, `.open_world` `:143`. `new` `:95`, `from_raw` `:100`, `with_annotations` `:216`. |
| C8 | IMPLEMENTATION_PLAN § 4 | "compare serialized JSON if `Tool` lacks `PartialEq`" | Dead hedge. `Tool` derives `PartialEq` (`rmcp-3.1.0/src/model/tool.rs:13`) and so does `ToolAnnotations` (`:50`). Verified directly. |
| C9 | TYPES § T7 / PLAN § 5a | `fixture_annotated_upstream_tool` 3-param vs 2-param, taking `&str` | Won't compile. Real fixture is `tests.rs:378` taking `&Arc<str>`. Reconcile to one signature. |
| C10 | SPEC § 6 rationale | "all four hints explicit" presented as convention | It **diverges** from the official filesystem reference server and FastMCP, which omit `destructiveHint`/`idempotentHint` on read-only tools. Defensible (determinism, and clients that read `destructiveHint` without checking `readOnlyHint` are common) but must be stated as a deliberate deviation. |
| C11 | SPEC § 1 | payoff = "clients pre-warn on destructive tools" | Over-stated. `readOnlyHint` is the hint with confirmed client behavior: VS Code skips confirmation, ChatGPT renders a READ/WRITE badge, and **Claude Code gates parallel tool execution** on it. No official client doc confirms `destructiveHint` producing a distinct stronger warning. The real payoff is concurrency + read/write rendering. |
| C12 | SPEC § 1 | only `with_annotations` call is at `catalog.rs:534` | `:535`. |

## 2. Answered open questions

| Q | Answer |
|---|---|
| Q1 — do the two mirror sites use byte-identical description literals? | **Yes.** `handlers_tools.rs:227` ≡ `peer_contract.rs:256`; `:241` ≡ `:268`. Confirmed independently by two agents. **No pre-existing hash bug.** Report as a clean negative result. |
| Q2 — which registry constructor for policy tests? | `build_docs_registry()` (`registry.rs:399`, `build_registry(false)`). `build_default_registry()` applies runtime conditions and omits `lab_admin` unless enabled locally, so the test would silently skip. |
| Q3 — generated artifacts needing regeneration? | None identified that embed tool JSON. Still run `just docs-generate && just docs-check`. |
| Q4 — ship the Code Mode stretch? | **No — cut.** See § 5. |

## 3. F9 — RESOLVED, accepted (2026-08-05)

> **Outcome: Option A — annotate all five, no amendment.** `can_execute` is
> default-deny on the absence of both `lab` and `lab:admin`
> (`call_tool_codemode.rs:807-815`); `lab:read` alone does not satisfy it.
> Operator confirmed no client holds a scope set lacking those two, and
> no-auth/stdio resolves to `TrustedLocal`. The widened reach is inert here.
> Phase 5e ships as a **regression guard**, since this rests on configuration
> rather than an invariant.

The mechanism, retained for the record:

**Finding (architecture round, engineering pass).** `UpstreamTool.destructive` is
not only an MRTR elicitation hint. It also gates:

- widget callbacks — `crates/labby/src/mcp/call_tool.rs:1175`
- the palette — `crates/labby-gateway/src/gateway/palette.rs:235-247`
  (`forbidden` unless `destructive_permitted` **and** `confirm_destructive`)
- Code Mode — `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs:90-107`,
  a hard `forbidden`, where
  `destructive_permitted(Mcp, c) == c.can_execute()`
  (`crates/labby-codemode/src/types.rs:800-804`)

So today a `lab:read` caller at hop 2 is **forbidden** from every proxied Labby
builtin. After annotation, `fs`, `server_logs`, `lab_admin`, `gateway_status`,
`doctor`, and `mcp_app` become **callable by a non-execute caller** — including
`mcp_app` enable/disable and `doctor.proxy.check`.

**Conflict, resolved.** The security engineering pass judged the relaxation "a
soft UX speed bump, not an authorization boundary." That assessment was formed
from the *given* framing ("relaxes MRTR elicitation") and did not account for the
three consumers above. The architecture finding cites specific code for each and
supersedes it. Treat F9 as an **authorization** change until a test proves
otherwise.

**Resolved as above.** The in-process tests still ship as a regression guard.

**Not implementable as specified:** the plan's `hop2_destructive` multihop
assertion. `mcp_multihop_conformance` is an out-of-process driver and
`UpstreamTool.destructive` never crosses the wire. Assert byte-identical
annotation survival there (which *is* observable) and move gating claims
in-process.

## 4. Test defects (all must be fixed)

| # | Defect | Fix |
|---|---|---|
| T1 | **The flagship Phase 4 test is vacuous.** `completion_test_registry()` (`tests.rs:133-155`) registers only two upstreams, and `code_mode_manager(true)` → `RootSynthetic` → `hides_raw_tools()` (`catalog.rs:51-53`) suppresses every builtin. ≥9 of 12 `EXPECTED` rows hit `continue` — green while proving nothing. | Build with `build_docs_registry()`, pass `code_mode_manager(false)` for `Raw` visibility, and **replace `continue` with coverage enforcement**: assert the found set equals the expected set. Add the inverse — every Labby-owned tool in `result.tools` must have `annotations.is_some()`. |
| T2 | **`unlisted_service_falls_back_to_least_safe` tests nothing** — it compares a const to its own literal and never calls `service_policy`. Nothing in the suite exercises the fail-closed fallback the design rests on. | Call `service_policy(&fake_unregistered_service())` and assert `LEAST_SAFE`. |
| T3 | No exhaustiveness check. A service added without a hint row silently ships `LEAST_SAFE` with zero CI signal; a stale row for a renamed service is equally silent. | Assert both directions: every registered service has a row, every row names a live service. |
| T4 | `read_only` is unfalsifiable from `ActionSpec`. A **mutating but non-destructive** action added to a read-only service passes every proposed test — the exact `doctor` trap, un-machine-checked. | Pin an action-name allowlist per read-only service (`fs` ⇒ `{fs.list}`, `server_logs` ⇒ `{server_logs.query}`, `lab_admin` ⇒ `{onboarding.audit}`) so any addition fails CI and forces a re-audit. |
| T5 | A mirror-equality test **already exists** at `tests.rs:1672-1683` (`assert_eq!(result.tools, contract_tools)`). | Extend that fixture; do not add a second test. |
| T6 | R6 is false above 100 tools — `list_tools_impl` truncates at the page cap, `visible_tool_descriptors` never paginates. The multihop harness already uses 75 tools/leaf. | Scope the R6 claim to below the page cap and document it. |
| T7 | Hard-coded hash baseline is a CI-churn generator (any description/schema edit flips it, diagnosed as two hex strings). | Replace with: hash the same descriptors twice in-process → equal; hash the set with vs without annotations → unequal. |
| T8 | `cached_upstream_tool_preserves_annotations_verbatim`'s `!cached.destructive` half duplicates `cached_upstream_tool_honors_explicit_non_destructive_hints` (`helpers.rs:600-630`). | Keep only the `annotations == Some(..)` half. |
| T9 | `derived_destructive_matches_decision_table` hardcodes the table a second time — the anti-pattern G2 exists to stop. | Share one `EXPECTED` const with the Phase 4 test, or drop it. |

## 5. Scope decisions

| Item | Verdict | Rationale |
|---|---|---|
| `_meta` per-action risk map | **CUT** (3 agents against, 1 for) | Duplicates `lab://<service>/actions` exactly where it is most valuable; a peer-accurate version would need ~122 async `RwLock` acquisitions per descriptor build; converts an allocation-free hot path into an allocating one; and raises hash-churn frequency from "never" to "every action rename". Also an information-disclosure risk — it would put the full admin-action inventory in every `tools/list`. |
| Code Mode stretch (`lab-g1av5.4`) | **CUT / wontfix** | Carries the package's only real cache stampede: `CatalogRenderCache` is a single `Arc<Mutex<Option<..>>>` slot with **no single-flight** (`manager.rs:134`) and `CatalogEmbeddingCache` shares its fingerprint, so its miss path issues batched TEI **network** calls. First burst after upgrade = N concurrent renders + N TEI round-trips. Counter-argument on record: `codemode`'s `destructiveHint: true` conveys nothing (rmcp's default is already `true`) and has no escape hatch — capture that in SPEC rather than by shipping `.4`. |
| `mcp/tool_schemas.rs` extraction | **DEFER** | Pure churn in the most contended file — five in-flight branches touch it. Phase 2 already edits the same lines; extracting after avoids a double diff. |
| Third mirror pair (`catalog.rs::add_server_app_available_on_mcp` vs `peer_contract.rs::add_server_app_available`) | **Separate bead** | Orthogonal visibility duplication; folding it in changes the advertised tool set. |
| `doctor` SSRF hardening | **Companion bead, land no later than `.2`** | See § 6. |
| Multihop test | **KEEP, reduced** | Keep annotation-survival only; drop the unimplementable gating assertion. |
| Subject-scoped OAuth passthrough test | **KEEP** | `pool/tools.rs:246-274` bypasses `UpstreamTool` entirely — a genuinely distinct path the plan itself calls most regression-prone. |
| Hash-determinism test | **REPLACE** (not defer) | See T7. |

## 6. Security conditions

1. **`doctor` SSRF — companion bead.** `dispatch/doctor/params.rs:94-113` is a
   weaker bespoke reimplementation of `labby-primitives/src/ssrf.rs`: permits
   `http`, hardcodes `is_blocked_ip = false` for `Host::Domain`, never applies
   `PRIVATE_TLD_SUFFIXES`, performs no DNS-resolution check. `proxy.check` is
   `destructive: false, requires_admin: false`, so **any** authenticated peer can
   call it on the primary gateway today — no chain required — and it returns a
   status/JSON/body-substring oracle across five probes. The epic both formalizes
   `openWorldHint: true` for it and removes a gate in front of it. Harden onto the
   canonical guard.
2. **`server_logs` needs an explicit exception.** `server_logs.query` is
   `requires_admin: true` (`dispatch/server_logs/catalog.rs:29`) and redaction is
   key-based only (`labby-runtime/src/redact.rs:29-64`) — free-text `message`
   values are never scanned. Under a static-credential upstream link every
   downstream caller executes as one fixed identity. Recommended: move
   `server_logs` out of the read-only bucket to
   `readOnlyHint: false, destructiveHint: true`, a documented override of R2
   (do **not** flip the action-level `ActionSpec.destructive`, which would add
   local MRTR/CLI friction everywhere). Widening redaction to scan free text is a
   good parallel follow-up, not a substitute.
3. **The "no `Tool::new` outside the descriptor module" invariant must be
   automated** — clippy `disallowed_methods`, an xtask lint, or equivalent. As a
   one-time manual grep it will not survive the five in-flight branches touching
   these files.
4. **Docs note:** tool visibility and `lab://<service>/actions` are scoped by
   `route_scope`, **not** by the caller's admin scope — action *metadata*
   (names, descriptions, `requires_admin`) crosses that boundary even though
   execution does not.
5. **`lab_admin` is read-only by vacuity** — `onboarding.audit` is declared but
   unimplemented (`dispatch/lab_admin/dispatch.rs:59-72` matches only
   `help`/`schema`). Tie a re-audit to its implementation.

## 7. Structural decisions

- **Home for shared builders: a new `crates/labby/src/mcp/descriptors.rs`, not
  `permanent_tools.rs`.** Four of five relocated descriptors are *conditionally
  advertised*, contradicting that module's permanence invariant, and importing
  `RegisteredService` there closes a `registry ↔ permanent_tools` cycle.
- **Simplify the policy module.** `ToolAnnotations` is `#[non_exhaustive]` and
  its builders are **not** `const fn` (`tool.rs:94-152`), so a plain-data
  intermediate is forced — but the `AnnotationPolicy` struct is one layer more
  than that requires. Use a free
  `to_annotations(read_only, destructive, idempotent, open_world)` plus a plain
  `match svc.name`.
- **Do not move hints onto `RegisteredService`.** A forgotten field would
  silently default instead of failing closed, destroying the R4/C8 property.
- **Do not cache the policy in a `LazyLock`/`OnceLock`.** A name-keyed process
  global would leak one registry's answer into another's —
  `build_default_registry` / `build_docs_registry` / test registries produce
  different service sets. Keep `service_policy` a pure function of its argument
  and say so in the module docs.
- **Keep the six per-tool descriptor builders.** The two mirror sites are
  asymmetric — one paginates and early-breaks, one builds the full list — so a
  single aggregate builder would be more complex, not less.
- **The hot path is `call_tool`, not `tools/list`.** `visible_tool_descriptors`
  runs twice per proxied upstream tool call
  (`call_tool_upstream.rs:265,417,427,481`; `call_tool_codemode.rs:559,602,608`),
  a ~1–30 ms function. The derivation adds ~250 ns over 122 `ActionSpec` entries —
  four to five orders of magnitude below the surrounding work.

## 8. Recommended bead re-split

Split `lab-g1av5.1` by **behavior change**, not by call site (splitting by call
site would ship a wire/hash desync):

- **`.1a` — provably no-op refactor.** Add `mcp/annotations.rs` (policy + unit
  tests, unwired) and `mcp/descriptors.rs`; rewire **both** mirror sites to the
  builders while they still return today's un-annotated `Tool`.
  **Acceptance: the contract hash is unchanged.** ~350 LOC, rebases cleanly
  against the five overlapping branches.
- **`.1b` — the semantic flip.** Turn annotations on inside the builders
  (~6 lines) plus the annotation/coverage/hash tests. ~200 LOC, tiny diff, large
  blast radius — and a clean revert point if F9 forces `mcp_app`/`doctor` to stay
  destructive. **F9 must be settled before this lands.**

Total post-simplification estimate ~600–650 LOC, within the 1000 budget.

## 9. Pre-existing issues found (separate beads, not this epic)

| Issue | Evidence |
|---|---|
| `gateway.test` spawns local processes but is `destructive: false` | `gateway/catalog.rs:527-529`; the root `CLAUDE.md` destructive policy names `gateway.test` explicitly. Arbitrary local code execution flagged non-destructive. |
| `doctor` SSRF validator weaker than canonical | § 6.1 |
| `docs/surfaces/MCP.md:66-68` contradicts the code | Says headless callers pass an explicit confirmation field; `mcp/CLAUDE.md:85-86` forbids exactly that, and the code agrees with CLAUDE.md. |
| Human/agent parity inverted | `help_payload` includes per-action `destructive` (`dispatch/helpers.rs:201`) but `render_catalog` (`output/render.rs:652-730`) drops it; the palette gives the desktop UI finer granularity than any agent gets. |
| No single-flight on `CatalogRenderCache` / `CatalogEmbeddingCache` | `manager.rs:134`, `code_mode.rs:130-138` |
| `evaluate_peers` is a sequential `for` with `.await` inside | `catalog_notifications.rs:336-366`; the sibling notify step correctly uses `join_all`. No memoization of identical contracts across peers. |
| `descriptor_contract_hash` does three deep traversals per descriptor, twice per proxied call | `catalog.rs:125-143` |
| `server_logs` redaction is key-based only | `labby-runtime/src/redact.rs:29-64` |

## 10. Confirmed correct (no change needed)

- MCP `2026-07-28` hint defaults and the "meaningful only when
  `readOnlyHint == false`" conditionality rule — exact match to the spec's
  `schema.ts`. `2026-07-28` is still the current revision.
- Upstream annotations already pass through verbatim; `.2` correctly adds tests
  rather than a fix. Dedup/filter logic is identical between the two sites.
- Local-hop authorization is untouched — `call_tool.rs:959-1014` consults only
  `ActionSpec.destructive`; only the proxied branch (`:1050-1064`) reads the
  annotation-derived value.
- Annotations are peer-independent: no `PeerCatalogAudience`, `auth`,
  `route_scope`, or `oauth_subject` reaches the policy functions. C6 determinism
  holds for Labby-owned tools (scope the claim to those — subject-scoped upstream
  tools legitimately vary per identity).
- The upstream-lying-about-`readOnlyHint` exposure is pre-existing and not
  widened by this epic.
- Pagination is count-based (`pagination.rs:4,34`), so larger descriptors cannot
  shift page boundaries.
- rmcp `=3.1.0` has no annotation-related deprecations; `#[tool(annotations(..))]`
  macro support exists and is orthogonal to the manual builders.
- The coarse-dispatcher-tool granularity problem is genuinely unsettled protocol
  territory (Tool Annotations Interest Group open question; SEP-1984 and SEP-1913
  unmerged), and confirmation fatigue is an acknowledged ecosystem-wide unsolved
  problem. Cite both rather than presenting them as gaps in this design.
