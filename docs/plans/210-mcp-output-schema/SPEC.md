# SPEC — MCP `outputSchema` + `structuredContent` (issue #210)

| | |
|---|---|
| Issue | [dinglebear-ai/labby#210](https://github.com/dinglebear-ai/labby/issues/210) |
| Epic bead | `lab-41e7m` |
| Branch | `feat/mcp-output-schema-210` |
| Base | `origin/main` @ `132448802` |
| MCP revisions | 2025-06-18 → 2026-07-28 (rmcp negotiates all four) |
| SDK | `rmcp` `=3.1.0` — no bump required (all 9 API claims verified) |
| Status | **Revised after 10-agent research pass.** Not implemented. |

> **Revision note.** This spec was rewritten on 2026-08-05 after a research pass
> materially changed its scope. The largest change: FR-1's schemas are visible **only in
> Raw mode** (§2.1). Superseded claims are recorded in [PROGRESS.md](PROGRESS.md) §7 rather
> than silently deleted.

---

## 1. Problem statement

Issue #210 asks for three things: declare `outputSchema` on directly-listed tools, preserve
`structuredContent` through Code Mode, and surface output shapes in the catalog for hidden
tools.

**Verification found most of it already implemented.** This spec describes the delta.

### 1.1 Already working (do not rebuild)

| Issue claim | Actual state | Evidence |
|---|---|---|
| Tools should emit `structuredContent` | Every dispatch result already sets it | `result_format.rs:111-114` (success), `:66-70` (error) |
| Code Mode should preserve it | Unwrap prefers it; `isError` checked first | `code_mode_host.rs:606-630`, `:494` — **byte-identical since 2026-05-31** (`977cb2166`) across three refactors |
| Catalog should carry output shapes | `ToolDescriptor.output_schema` populated from upstreams | `types.rs:107`, `search.rs:135-141` |
| Output shapes should reach the model | `.d.ts` renders `Promise<T>` | `ts_signatures.rs:32-71` |
| Some tools declare `outputSchema` | `codemode` + `codemode_ui` already do | `handlers_tools.rs:204`, `peer_contract.rs:235` |

### 1.2 The actual gaps

**G1 — Builtin and synthetic tools declare no `outputSchema`.** `handlers_tools.rs:139`
(builtins), `:212` (`mcp_app`), `:225` (`add_server`), `:239` (`gateway_status`).

**G2 — Descriptor and gating logic is duplicated across FOUR sites, not two.**

| # | Site | Decides |
|---|---|---|
| 1 | `handlers_tools.rs:52-334` | paginated live-request tool list |
| 2 | `peer_contract.rs:190-310` | unpaginated descriptor set for `list_changed` diffing |
| 3 | `catalog.rs:217-270` | `add_server_app_available_on_mcp`, `gateway_status_app_available_on_mcp`, `action_allowed_on_mcp` |
| 4 | `peer_contract.rs:117-146` | the same three booleans, hand-duplicated |

Sites 3/4 gate **authorization**, not just schema shape, making them the more dangerous pair.
`catalog.rs:218-286` already demonstrates the correct delegation pattern for
`current_upstream_pool`, `service_visible_on_mcp`, and `code_mode_visibility` — the codebase
knows how and simply did not apply it to the admin-tool gates.

History confirms this is not hypothetical: `peer_contract.rs` was created 2026-07-25
(`e617a22c3`) as a parallel re-implementation whose own doc comment says it "mirrors the
handler". **That same commit fixed one instance of this exact drift class** for
`code_mode_upstreams_for_description`, with the message *"otherwise a change to either drifts
silently."* The rest was left un-consolidated.

**G3 — The unwrap contract is correct but unpinned** by tests or docs.

**G4 — The Code Mode error trace omits `logs_count`**, which its advertised schema requires
(`handlers_tools.rs:755`). Dated precisely: the field became required 2026-06-08
(`68cc3b8aa`); the error path was added 2026-07-11 (`bf75ed4ac`) without it. Live and untested
for 25 days. **This is not a conformance violation** — see FR-7.

**G5 — Catalog coverage has two open ends**: snippets (`types.rs:186`, deliberate) and
OpenAPI-derived output schemas (unverified).

**G6 — Two security defects found during research** (§3.6), neither created by this issue but
both in its blast radius.

---

## 2. Scope reality — read before implementing

### 2.1 FR-1 is a Raw-mode-only improvement

Under `hide_raw_tools` — true whenever Code Mode is enabled (`catalog.rs:51-53`,
`peer_contract.rs:86-91`) — **every builtin service tool except `server_logs` is suppressed
from `tools/list`** (`handlers_tools.rs:135-137`, `peer_contract.rs:201-203`).

Consequences the original spec missed:

- FR-1's schemas on `gateway`, `doctor`, `setup`, `fs`, `snippets`, `lab_admin` are advertised
  **only when Code Mode is off**.
- In the flagship Code Mode deployment, this change adds **one** schema (`server_logs`).
- The drift test still passes, because both builders skip the same tools.
- Builtins are additionally **unreachable** in Code Mode: `lab::*` IDs are explicitly barred
  (`call_tool_codemode.rs:308-309`), and no builtin enters the Code Mode catalog
  (`search.rs:126-148`). The tool description redirects agents to "native Lab service tools" —
  which are the suppressed ones.

**Decision:** proceed, scoped and labelled honestly as a Raw-mode improvement. Exposing
builtins through the Code Mode catalog is a materially larger change and is filed as a
follow-up (§6 FU-1). **Do not ship this described as "output shapes now reach agents."**

### 2.2 Wire cost is negligible

`PageCollector` budgets by **count** (`MCP_LIST_PAGE_SIZE = 100`, `pagination.rs:4,35`), not
bytes, so `outputSchema` cannot shrink pages. Added payload: ~438 B/tool → **~3.1 KB** at
all-features (7 builtins), **~438 B** under `hide_raw_tools`. Flat as upstreams scale.

### 2.3 The real stake is the contract hash

`descriptor_contract_hash` (`catalog.rs:125-143`) serializes the whole `Tool`, so
`output_schema` is inside the digest driving `tools/list_changed`. One-sided drift does not
merely hide a schema — it makes change detection **structurally wrong**: patch only
`handlers_tools` and the hash never moves for a set that did; patch only `peer_contract` and
every peer gets a spurious notification.

> **Correction (was wrong in the first revision).** An earlier draft claimed landing this change
> causes a one-time `tools/list_changed` fanout on upgrade, and instructed `.4` to document it.
> **It does not.** `server.rs:531-541` seeds each subscription's baseline with the contract that
> peer actually received, and `PeerRegistry` is `Arc<RwLock<Vec<RegisteredPeer>>>`
> (`peers.rs:147`) — purely in-memory. An upgrade restarts the process, every transport dies, and
> new sessions register with the new descriptor set as their baseline. **Fanout on upgrade: zero.**
> The only exception is Labby-proxying-Labby, where instance A sees B's tools change — the
> ordinary upstream-change path, already coalesced and deferred past open turns
> (`catalog_notifications.rs:110`, `:132`). Do not write the original claim into permanent docs.

---

## 3. Functional requirements

### FR-1 — Envelope output schema for builtin service tools

Builtin service tools whose success results flow through `format_dispatch_result` MUST declare
the success-envelope schema via `Tool::with_raw_output_schema`.

```json
{ "ok": true, "service": "gateway", "action": "gateway.list", "data": { } }
```

`data` is unconstrained (NG-1). Scope limits per §2.1.

### FR-2 — One builder, extending the existing seam

**`PermanentToolRegistry` (`permanent_tools.rs:56-77`) is already this pattern** — a
registry-owned descriptor constructor calling `Tool::new(...).with_raw_output_schema(...)`,
already consumed by both call sites (`handlers_tools.rs:166-169`, `peer_contract.rs:222-225`).
Labby-owned descriptor construction MUST be extended there rather than in a new parallel
module. Signature MUST take the per-caller axis only:

```rust
pub(crate) fn builtin_service_tool(service: &RegisteredService, admin_apps_visible: bool) -> Tool
```

The invariant `name == SERVER_LOGS_TOOL_NAME` test lives inside; only the audience bool differs
between callers.

**FR-2a — Gating consolidation. MOVED TO ITS OWN BEAD; do not implement inside `.1`.**

`add_server_app_available*`, `gateway_status_app_available*`, and `action_allowed_on_mcp` are
duplicated across `catalog.rs:217-270` and `peer_contract.rs:117-146` and MUST collapse to one
implementation. Three constraints, all load-bearing:

- **Own bead.** The gates have **8** call sites and only 2 are in `.1`'s files: `call_tool.rs:461`,
  `:494`, `:603`, `:679` (every builtin dispatch) and `handlers_resources.rs:408`, `:434`,
  `:1071`, `:1170`. Bundling an authorization refactor into a bead titled "envelope schema"
  means a reviewer approving schema work implicitly approves a change to who can see
  `add_server`/`gateway_status`, and a revert of one drags the other.
- **The consolidated gate MUST remain audience-free.** *(HIGH — this is the most dangerous edit
  the epic makes possible.)* Today the audience factor is applied **outside** the gate bodies
  (`peer_contract.rs:253`, `:265`; `handlers_tools.rs:124-125`; `call_tool.rs:461`), which is
  the only reason delegating through `self.peer_contract()` is safe — that constructor supplies
  `PeerCatalogAudience::default()` with `admin_apps_visible: true` (`peer_contract.rs:49-58`).
  Folding the audience check *into* the shared body would silently grant `add_server` and
  `gateway_status` to unprivileged callers at four dispatch and resource sites. Require a test
  asserting non-admin **denial** at the dispatch and resource paths, not just at `tools/list`.
- **Shape it as a free function, not a `PeerContract` method reached via `self.peer_contract()`.**
  A free function over `(Option<&Arc<GatewayManager>>, &McpRouteScope, &ToolRegistry)` gives one
  definition with zero new `PeerContract` constructions. Routing `LabMcpServer`'s delegates
  through `self.peer_contract()` would put a deep `McpRouteScope` clone on `call_tool.rs:679` —
  i.e. every builtin dispatch — which is exactly the allocation pattern FU-2 defers.

Net effect when done correctly: lock counts unchanged, `PeerContract` constructions per
`tools/list` drop 11 → 9. Preserve one real divergence: `catalog.rs:249-252` uses
`registry.services().iter().any(...)` where `peer_contract.rs:133` uses
`registry.service("gateway").is_some()` — confirm equivalence before assuming it.

### FR-3 — Audit before attachment (two axes)

Before attaching any schema, each directly-listed tool MUST be audited for **both**:

1. **Shape** — does every success path produce the envelope?
2. **Presence** — does every success path set `structuredContent` *at all*?

Axis 2 is new and load-bearing: the **Python SDK hard-errors** when a tool declares
`outputSchema` but returns no structured content ("Tool X has an output schema but did not
return structured content"), and this exact failure already broke Claude Code's own Bash tool
in production ([anthropics/claude-code#14465](https://github.com/anthropics/claude-code/issues/14465)).

### FR-4 — Code Mode unwrap contract

Document and test the existing precedence (unchanged since 2026-05-31). See
[CONTRACT.md](CONTRACT.md) §C6.

### FR-5 — Structured content survives truncation

**There are two truncation markers; the original spec cited the wrong one.**

| Path | Shape | Default? |
|---|---|---|
| `truncate.rs:178-192` | object: `{truncated: true, original_size, original_tokens, preview, artifacts, next_action}` | **yes — always-on outer budget net** |
| `shape.rs:99-135` | plain string `"[code mode result truncated]…"` | no — only under non-`Off` result-shape policy (`Off` is `#[default]`) |

Both MUST be covered. Shaping MUST NOT re-serialize a surviving structured value.

### FR-6 — Upstream proxy fidelity (success path only)

A **successful** upstream `CallToolResult` relayed downstream MUST retain `structuredContent`
byte-identically. **Error results are explicitly excluded**: `enrich_completed_tool_error_result`
(`upstream/tool_error.rs:148-180`) deliberately rewraps them as
`{"error": <contract>, "upstream_structured_content": <original>}` — a published,
schema-locked contract introduced by `3e5ab3df`.

Confirmed new work: `67a335ad` touched only the error path; its success branch is byte-identical
to its parent, and no test asserts success-path fidelity today.

### FR-7 — Trace schema self-consistency (reason corrected)

Add `logs_count: 0` to the error trace (`call_tool_codemode.rs:659-670`).

**Rationale is internal consistency for trace consumers, NOT conformance.** The error trace is
attached to `CallToolResult::error(...)` → `isError: true`, which D1/§5.1 hold to be exempt.
Claiming a conformance violation here would contradict this spec's own central decision. The
real reason: the inline inspector widget consumes `structuredContent` on both paths, and the
success path always supplies the field.

### FR-8 — Catalog coverage

Upstream `output_schema` flows to `describe` and `.d.ts`; snippets keep `None` with a comment;
OpenAPI-derived schemas audited (implement if ≤150 LOC, else record and defer).

### FR-9 — Security defects (new)

**FR-9a (HIGH).** Upstream `description`, `inputSchema`, and `outputSchema` reach `tools/list`
**completely unsanitized** (`handlers_tools.rs:288`, `peer_contract.rs:290`;
`cached_upstream_tool` stores them verbatim at `upstream/pool/helpers.rs:420-448`).
`sanitize_tool_text`/`sanitize_schema` (`projection.rs:57-155`) run **only** on the Code Mode
catalog path. A malicious upstream can inject prompt-injection payloads into any direct
client's context via schema `description` fields at any nesting depth. Pre-existing, but this
issue formalizes verbatim relay — it must not be codified without a carve-out.

> **The obvious fix is a trap. `sanitize_schema` is not schema-keyword-aware.** Its `recurse`
> (`projection.rs:127-144`) runs *every* JSON string in the tree through `sanitize_tool_text`:
> deleting `<system>`/`[INST]`/`###`/`<<` substrings, redacting `SECRET_REGEX` matches, and
> truncating at 2048 chars. That is right for `description`/`title` and **wrong** for `enum`,
> `const`, `default`, `pattern`, `format`, and `$ref`, which are load-bearing semantics. A
> legitimate enum literal `"ghp_deploy_tag"` becomes `[REDACTED]`; a `pattern` containing `###`
> as regex syntax is silently rewritten. Because FR-6/C5.2 keep the *result* byte-identical, the
> advertised schema and the real payload then disagree — and a strict client rejects a
> **conforming** upstream result. That is the Claude Code #14465 failure class, self-inflicted.
>
> Requirements: sanitize only documentation-bearing keys; never touch `enum`/`const`/`default`/
> `pattern`/`format`/`$ref`. Keep `sanitize_schema`'s 512 KB size gate where it is rather than
> moving it to cache time — `projection.rs:117-123` records that an earlier 16 KB gate already
> collapsed legitimate cortex/axon schemas to `unknown` and was reverted. Ensure idempotency, since
> the Code Mode catalog path already sanitizes and would otherwise double-truncate. Test: an
> upstream schema whose `enum` value looks like a secret survives verbatim while its
> `description` is stripped.

**FR-9b (HIGH). Blocking for `.3`, not merely parallel.** `schema_to_type`
(`ts_signatures.rs:77-202`) caps recursion **depth** at 20 (`:90-91`) and removes `seen_refs` on
return (`:105`). Non-cyclic shared `$ref` reuse yields O(B^depth): B=3 gives ~3.5 billion
expansions from a few-KB schema, well under `MAX_SCHEMA_BYTES = 524_288`.

- **Use a node/iteration budget — and a byte budget. Do NOT memoize per `(ref, root)`.** That
  alternative is wrong three ways: expansion depends on the *current* `seen_refs` set, not on
  `(ref, root)`, so a cached entry returns a cycle-truncated result where full expansion was
  correct (and is traversal-order dependent — an unrelated schema edit flips which occurrence
  wins); it does not cover the composition arms (`:127-144` recurses over every
  `anyOf`/`oneOf`/`allOf` element with no `$ref` involved); and it bounds no output. The
  returned `String` is built by concatenation, so at B=3/depth 20 the *result* is O(3²⁰) **bytes** —
  a multi-gigabyte allocation and an OOM kill, not a slow request.
- **Emit `tracing::warn!` on exhaustion** naming upstream and tool. Silent degradation to
  `unknown` is precisely the regression `projection.rs:117-123` documents having shipped before.
- **Thundering herd amplifier:** the vulnerable path sits behind a *single-slot* cache
  (`Arc<Mutex<Option<..>>>`, `manager.rs:133`) whose rebuild runs **outside** the lock
  (`search.rs:122`+). N concurrent sessions on a cache miss each run the exponential expansion,
  and a hostile upstream forces the miss on demand by churning its tool list. Since `.3` adds
  tests that push upstream `output_schema` through `generate_tool_types`, land FR-9b first.

---

## 4. Non-functional requirements

| ID | Requirement |
|---|---|
| NFR-1 | No `mod.rs`. |
| NFR-2 | Native `async fn` in trait; no `#[async_trait]`. |
| NFR-3 | `ToolError` serialization stays hand-written. |
| NFR-4 | Never stringify-and-reparse structured data. |
| NFR-5 | Schema content entering the Code Mode catalog goes through `sanitize_schema`. |
| NFR-6 | New/renamed error kinds need variant + `IntoResponse` arm + `docs/dev/ERRORS.md` together. None expected. |
| NFR-7 | All-features build is the verification truth. |
| NFR-8 | Generated docs are code-owned. **Note:** `.1` will not break `docs-check` — generated artifacts render from the registry action catalog (`docs/render.rs:18`), not from `rmcp::model::Tool`, so no artifact contains `outputSchema`. |
| NFR-9 | Static schemas via `LazyLock<Arc<JsonObject>>`, shared by `Arc::clone`. |
| NFR-10 | Published schemas follow repo convention: `docs/contracts/schemas/*.schema.json`, `$id` `https://dinglebear.ai/schemas/labby/<name>-v1.json`, with a drift test reading the file as plain JSON (pattern: `crates/labby-runtime/tests/agent_error_schema.rs`). No schema-validation dependency. |

---

## 5. Design decisions

### 5.1 Success-only schema — correct, but the justification needed fixing

**Decision.** `outputSchema` describes the success envelope only.

**Corrected rationale.** The original spec cited MCP 2025-06-18 as *exempting* `isError`
results. **No such spec text exists.** The exemption is a converged ecosystem convention, and
implementations are still being retrofitted to honor it:

- The official **TypeScript SDK client** validates error envelopes against the success
  `outputSchema` and throws `-32602`; the guard is an open PR
  ([typescript-sdk#1945](https://github.com/modelcontextprotocol/typescript-sdk/pull/1945)).
  Its server side already has the guard — a client/server asymmetry.
- A gateway ([IBM/mcp-context-forge#4202](https://github.com/ibm/mcp-context-forge/issues/4202))
  corrupted error messages the same way before adding an `is_error` early return.

The decision stands — widening the schema to cover errors was explicitly rejected upstream as
violating settled semantics — but the *hazard is real and client-side*, not theoretical.

**Repo-local reinforcement.** `3e5ab3df` made the completed-error envelope a published,
schema-locked contract that **wraps** rather than preserves a tool's success-shaped
`structuredContent`. Declaring one schema for both would conflict with that locked contract.

### 5.2 One shared generic schema

Single static, `service: { "type": "string" }`, no per-service `const`. Preserves the shared
`Arc`; a `const` adds nothing the envelope's own `service` field lacks.

**`additionalProperties`: use `true`. Decided.** An earlier draft left this as a tiebreak
("if in doubt, prefer `true`") while CONTRACT and the plan's code sample both hard-coded
`false` — a contradiction an implementer would have resolved by shipping `false`. Resolved in
favour of `true` for three reasons:

1. Closing the envelope is the textbook `PROPERTY_ADDED_TO_OPEN_CONTENT_MODEL` forward-compat
   break: one future field breaks **all seven** builtins' advertised schemas simultaneously,
   client-side, on every success.
2. That same field also moves `descriptor_contract_hash` (`catalog.rs:125-143`), so the break
   arrives coupled to a `tools/list_changed` for every peer.
3. This envelope family demonstrably grows — the *error* envelope already gained a versioned
   recovery contract (`envelope.rs:57-66` → `build_agent_error_value`).

The only thing `false` buys is detecting a malformed envelope Labby itself produced, and AC-3's
"exactly four keys" assertion gives that **internally**, without exporting brittleness to
clients. Keep the comment at `envelope.rs:42` binding `build_success` and the schema to the same
commit.

### 5.3 No protocol-version gating

Advertise unconditionally. rmcp serializes `output_schema` regardless of negotiated version
(verified); older clients ignore unknown fields.

**Revision note.** SEP-2106 (2026-07-28) loosened `structuredContent` from object-only to *any*
JSON value and `inputSchema`/`outputSchema` to any JSON Schema 2020-12 keywords. Labby's
envelope is an object, so this relaxes rather than constrains us — but "2025-06-18 semantics"
is a stale label since rmcp negotiates up to 2026-07-28.

### 5.4 Unwrap: lock, don't change

Confirmed byte-identical since `977cb2166` (2026-05-31) across three refactors. No regressions
in ~10 weeks.

### 5.5 Truncation stays at the outer boundary

Confirmed. See FR-5 for the two-marker correction.

### 5.6 Extend existing seams, do not invent parallel ones

Applies to both the builder and the drift test (§6 AC-2). A new `tool_descriptors.rs` would
split "where do Labby-owned descriptors come from?" across two modules.

**Stated precisely, because an earlier draft overstated it.** `PermanentToolRegistry` is *not*
already a builtin-descriptor factory. It is a unit struct (`permanent_tools.rs:38`) fronting
`const PERMANENT_TOOLS: [PermanentToolEntry; 1]` (`:32-35`) whose purpose is
`resolve(name) -> PermanentToolId` — dispatch identity that survives upstream churn. Builtin
service tools come from a different registry (`ToolRegistry::services()`) and will never appear
in `PERMANENT_TOOLS` or resolve through it. What is true: **the module exists, and the pattern
(registry-owned `Tool` + `with_raw_output_schema`, called from both sites) is precedented there
exactly once**, by `code_mode_descriptor` (`:56-77`).

So the choice is: add the builtin/synthetic constructors to `PermanentToolRegistry`, **or** create
a descriptor module and move `code_mode_descriptor` into it leaving `permanent_tools.rs` owning
only identity/`resolve`. **Ship one, not both.** Recommendation: the former, and in the same
commit rewrite the module doc — currently *"Product-level MCP tools whose identity and dispatch
exist independently of upstream health"* — to name both responsibilities. A stale doc there is
how the next reader re-invents `tool_descriptors.rs`.

---

## 6. Acceptance criteria

| ID | Criterion |
|---|---|
| AC-1 | Every audited builtin advertises the envelope schema, asserted by content. |
| AC-2 | **Extend the existing drift test** (`handlers_tools/tests.rs:1671-1684`, which already asserts full `Tool` equality between `tools/list` and the peer contract) with a **Raw-mode fixture**. Today it runs only under `hide_raw_tools`, where builtins are suppressed — so it never exercises the loop being changed. Do not add a weaker field-by-field test. |
| AC-2a | Snapshot parity: `ToolCatalogSnapshot::from_descriptors(&result.tools) == snapshot_tool_catalog_for_request(&ctx)`. |
| AC-3 | Success `structuredContent` conforms **and is always present** (FR-3 axis 2), including `help` and `schema` built-ins. |
| AC-4 | Synthetic tools carry an accurate schema or none, per audit. |
| AC-5 | Schema present regardless of pagination position. (Low risk — count-based paging.) |
| AC-6 | On builtin/upstream collision, advertised schema matches the answering tool. |
| AC-7 | Unwrap edge matrix: falsy structured values, structured + resource, multi-block text (joinable and not), empty content, mixed content, `isError` precedence, and `_meta` exposure on the mixed-content fallback. |
| AC-8 | **Both** truncation markers covered (FR-5), including the default `truncate.rs` path. |
| AC-9 | Error trace consistent with the advertised trace schema. |
| AC-10 | **Success-path** relayed `structuredContent` byte-identical; error-path rewrapping explicitly asserted as intended. |
| AC-11 | `describe` carries upstream schema; typed `Promise<T>`; absent → `unknown`, never fabricated. |
| AC-12 | nextest + clippy clean at all-features. |
| AC-13 | `just docs-check` clean. (Expected no-op — NFR-8.) |
| AC-14 | `MCP.md` + `CODE_MODE.md` updated; `ERRORS.md` drift-free. |
| AC-15 | Published schema drift test follows the `agent_error_schema.rs` pattern (NFR-10). |
| AC-16 | Docs state plainly that FR-1 is Raw-mode-only (§2.1) — and the caveat is repeated in the PR description and the issue comment, not only in this package. |
| AC-17 | **All four synthetic-tool constructors** (`codemode_ui`, `mcp_app`, `add_server`, `gateway_status`) are migrated to the shared builder. Without this, CONTRACT §C8.7 is false for those tools and the duplicated description literals (`handlers_tools.rs:227` ≡ `peer_contract.rs:256`; `:241` ≡ `:268`) remain live drift surface. Nothing else in the AC list gated this, so it was deprioritizable. |
| AC-18 | The Raw-mode drift test is **hermetic**: it must not depend on `process_code_mode_enabled()`, a process-global `AtomicBool` (`config.rs:64-66`) that any other test can set. Use `McpRouteScope::protected_subset(..., expose_code_mode: false)` with a populated services list, which forces `Raw` unconditionally (`peer_contract.rs:78`). The repo already solves this hazard the same way (`peers.rs:186-188`). |

### 6.1 Non-goals

- **NG-1** Per-action output schemas. `data` stays open.
  **Correction:** the original mitigation ("use the `schema` built-in action") is **hollow**.
  That action returns `ActionSpec.returns`, a `&'static str` documented as *"not a runtime
  contract — purely informational"* (`labby-primitives/src/action.rs:42-44`), with live values
  like `"DoctorReport"` that resolve to nothing. **There is currently no path by which an agent
  learns an action's result shape.** State this as a limitation, not a mitigation. If per-action
  schemas are ever built, `ActionSpec.returns` is the seam to grow — do not invent a third
  return-shape vocabulary.
- **NG-2** No `rmcp` bump. **NG-3** No version gating. **NG-4** No unwrap behavior change.
  **NG-5** No envelope shape change. **NG-6** No inbound `list_changed` listener.
- **NG-7** Builtins are not added to the Code Mode catalog (that is FU-1).

### 6.2 Follow-ups (file as beads, do not fold in)

| ID | Item | Source |
|---|---|---|
| FU-1 | Expose builtin dispatch envelope through the Code Mode catalog so schema and capability arrive together | §2.1 |
| FU-2 | Hoist one `PeerContract` in `list_tools_impl` — constructed **11×** per call (verified: 1 visibility + 7 services + 2 app gates + 1 pool). Cost applies to **`ProtectedSubset` routes only** — `Root` is a unit variant (`route_scope.rs:6-15`), so the clone is nearly free there. FR-2a already takes 11 → 9 | perf |
| FU-3 | **Re-graded to P1 — the dominant cost in this subsystem.** `snapshot_tool_catalog_for_request` (`catalog.rs:457-464`) is a *full catalog rebuild*, and `descriptor_contract_hash` materializes every descriptor 3× (`:129-131`). Invoked **twice per proxied tool call** (`call_tool_upstream.rs:265`, `:417`, `:427`, `:481`) and twice per Code Mode execution (`call_tool_codemode.rs:559`, `:602`, `:608`); the notification fanout re-derives **sequentially** per peer (`catalog_notifications.rs:341-364`) | perf |
| FU-4 | `upstream_tool_last_error` O(U) async lock loop **per page** (not per request — it sits after the `break` at `handlers_tools.rs:296`), for one tracing field | perf |
| FU-5 | Local providers (`state::*` ~28 methods, `git::*`, `openapi::*`) are absent from the catalog *and* from "Available globals" in the tool description — act-without-see | agent-native |
| FU-6 | Duplicated upstream-merge loops (~50 lines) between the two builders | pattern |
| FU-7 | `ActionSpec.returns` → real JSON Schema (unblocks NG-1). **Forward constraint:** FR-1's "no new disclosure" finding holds only while NG-1 stands; promoting per-action schemas exposes internal action/parameter shape to any Raw-mode-visible client and needs its own gating review | agent-native |
| FU-8 | Publish `code-mode-trace.schema.json` as a `docs/contracts/` artifact — deferred; this issue touches that schema only with a one-line fix, and the Rust schema already ships and is covered | scope |

---

## 7. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Advertising a schema a success path then violates — **Python SDK hard-errors** | **High** | FR-3, both axes |
| Unsanitized upstream schema text on `tools/list` | **High** | FR-9a |
| `$ref` expansion DoS | **High** | FR-9b |
| Four-site drift, incl. authorization gates | **High** | FR-2, FR-2a, AC-2 |
| TS SDK client validating error results | Medium | Out of our control; do not widen the schema |
| `additionalProperties: false` forward-compat | Medium | §5.2 + C3.5 |
| Shipping this as "output shapes now reach agents" | Medium | AC-16 |
| One-time `list_changed` fanout on upgrade | Low | Note in `.4` |

## 8. Traceability

| Requirement | Bead |
|---|---|
| FR-1, FR-2, FR-3, FR-7, AC-17, AC-18 | `lab-41e7m.1` |
| FR-4, FR-5, FR-6 | `lab-41e7m.2` |
| FR-8 | `lab-41e7m.3` — **blocked by FR-9b** |
| NFR-8, AC-13, AC-14, AC-16 | `lab-41e7m.4` |
| **FR-2a** | **new bead** — authorization blast radius, own revert unit (§FR-2a) |
| FR-9a, FR-9b | **new beads required**; FR-9b blocks `.3` |

**Constraint:** `code_mode_trace_output_schema` MUST stay in `handlers_tools.rs` for this epic —
`.2` asserts against it while `.1` refactors that file.

## 9. References

[CONTRACT.md](CONTRACT.md) · [SCHEMAS.md](SCHEMAS.md) · [MODELS.md](MODELS.md) ·
[TYPES.md](TYPES.md) · [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) ·
[RESEARCH.md](RESEARCH.md) · [PROGRESS.md](PROGRESS.md)
