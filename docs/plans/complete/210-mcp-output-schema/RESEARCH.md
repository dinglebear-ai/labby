# RESEARCH — findings from the 10-agent pass (2026-08-05)

Evidence behind the SPEC/CONTRACT/PLAN revisions. Ten domain-matched agents reviewed the
plan package against the real tree, the vendored `rmcp` 3.1.0 source, the MCP specification,
git history, and institutional memory.

Findings that **changed the plan** are marked ⚠. Findings that **confirmed** it are marked ✓.

| Agent | Verdict |
|---|---|
| architecture-strategist | ⚠ Two proposals reinvent existing seams |
| code-simplicity-reviewer | ⚠ Package ~10:1 oversized vs the diff |
| best-practices-researcher | ⚠ Spec citation wrong; real client hazards |
| framework-docs-researcher | ✓ All 9 rmcp API claims correct |
| learnings-researcher | ✓ Drift class documented as chronic |
| security-sentinel | ⚠ Two HIGH defects |
| performance-oracle | ✓ Plan perf-safe; 3 adjacent issues |
| pattern-recognition-specialist | ⚠ Four duplicated sites, not two |
| agent-native-reviewer | ⚠ **Scope inversion** — schemas invisible in Code Mode |
| git-history-analyzer | ✓ FR-6 new work; FR-7 dated; unwrap stable |

---

## 1. ⚠ Scope inversion — the highest-impact finding

**The envelope `outputSchema` is invisible in the mode this issue exists to serve.**

`hide_raw_tools` (true whenever Code Mode is enabled — `catalog.rs:51-53`,
`peer_contract.rs:86-91`) suppresses every builtin service tool from `tools/list` except
`server_logs` (`handlers_tools.rs:135-137`, `peer_contract.rs:201-203`).

- FR-1's schemas reach clients **only when Code Mode is off**.
- Under Code Mode the change adds exactly **one** schema.
- The drift test passes regardless — both builders skip the same tools.

Compounding it, builtins are **unreachable** under Code Mode: `lab::*` IDs are barred
(`call_tool_codemode.rs:308-309`), no builtin enters the catalog (`search.rs:126-148`), and the
tool description redirects agents to "native Lab service tools" — the suppressed ones. An agent
asked to "list my gateway upstreams" has no discoverable path.

→ SPEC §2.1, AC-16, FU-1.

## 2. ⚠ Both central proposals reinvent existing machinery

**2a. The builder seam exists.** `PermanentToolRegistry` (`permanent_tools.rs:56-77`) is
already a registry-owned descriptor constructor calling
`Tool::new(...).with_raw_output_schema(...)`, already consumed by both call sites
(`handlers_tools.rs:166-169`, `peer_contract.rs:222-225`). A new `tool_descriptors.rs` would
split the answer to "where do Labby-owned descriptors come from?" across two modules.
*(architecture + pattern agents, independently.)*

**2b. A stronger drift test already ships.** `handlers_tools/tests.rs:1671-1684`:

```rust
assert_eq!(
    result.tools, contract_tools,
    "tools/list and the notification contract must use identical descriptors"
);
```

Full structural `Tool` equality — covers `_meta`, annotations, title, icons, everything the
planned field-by-field test would have missed. **The gap is coverage, not existence**: it runs
only in the code-mode fixture where builtins are suppressed.

**2a-bis. Refined by the engineering review (see PROGRESS §6a).** The claim above is *half*
true and was restated too strongly in the first revision. `PermanentToolRegistry` is a unit
struct fronting `const PERMANENT_TOOLS: [PermanentToolEntry; 1]` whose job is
`resolve(name) -> PermanentToolId`; builtin service tools come from `ToolRegistry::services()`
and never enter it. What is precedented there is the *pattern* — a registry-owned constructor
calling `Tool::new(...).with_raw_output_schema(...)` from both call sites — exactly once, via
`code_mode_descriptor`. The practical conclusion (add the constructors there rather than
creating a parallel module) stands; the framing does not. Ship that **or** a descriptor module
owning `code_mode_descriptor`, never both, and rewrite the module doc in the same commit.

**2c. Signature correction.** `builtin_service_tool(service, server_logs_meta)` leaves the
invariant `name == SERVER_LOGS_TOOL_NAME` check duplicated at both callers. Correct seam:
`(service, admin_apps_visible: bool)`.

→ SPEC FR-2, AC-2, §5.6.

## 3. ⚠ Four duplicated sites, not two

Beyond the two descriptor builders, `catalog.rs:217-270` and `peer_contract.rs:117-146`
hand-duplicate the same five-condition chains for `add_server_app_available*`,
`gateway_status_app_available*`, and `action_allowed_on_mcp`. **These gate authorization.**

Intentional vs accidental, precisely separated:

| Divergence | Verdict |
|---|---|
| `server_logs` meta / codemode gates via `PeerCatalogAudience` | **Intentional** — a stored contract must re-evaluate without a live `RequestContext` |
| `add_server` / `gateway_status` gates | **Drift** — no timing dependency, two hand-written bodies |
| `LazyLock` vs per-call `action_schema()` | **Allocation difference only** — same value; *not* drift evidence |
| `builtin_names` `Vec<&str>` vs `HashSet<String>` | Accidental, harmless today |
| Redundant `route_scope` check at `handlers_tools.rs:131` | Dead redundancy — `service_visible_on_mcp` already checks it |

`catalog.rs:218-286` already delegates correctly for three other methods — the pattern exists
and simply was not applied here.

→ SPEC G2, FR-2a.

## 4. ⚠ Specification citation was wrong

**No spec text exempts `isError` results from `outputSchema` conformance.** The normative
sentence is identical across 2025-06-18 / 2025-11-25 / 2026-07-28:

> "If an output schema is provided: Servers MUST provide structured results that conform to
> this schema. Clients SHOULD validate structured results against this schema."

The exemption is converged **convention**, still being retrofitted:

- TS SDK **client** validates error envelopes against the success schema and throws `-32602`;
  guard is open PR [typescript-sdk#1945](https://github.com/modelcontextprotocol/typescript-sdk/pull/1945).
  Server side already guards — a client/server asymmetry.
- [IBM/mcp-context-forge#4202](https://github.com/ibm/mcp-context-forge/issues/4202) corrupted
  error messages identically before adding an `is_error` early return.
- Widening the schema to cover errors was explicitly **rejected** upstream as violating settled
  semantics ([cyanheads/mcp-ts-core#241](https://github.com/cyanheads/mcp-ts-core/issues/241)).

**SEP-2106 (2026-07-28)** loosened `structuredContent` to any JSON value and the schemas to any
JSON Schema 2020-12 keywords — relaxing, not constraining, Labby's object envelope.

→ SPEC §5.1, §5.3; CONTRACT §C3.2.

## 5. ⚠ Declaring a schema is riskier than the plan stated

**The Python SDK hard-errors** when `outputSchema` is declared but `structuredContent` is
absent: *"Tool X has an output schema but did not return structured content."* This already
broke **Claude Code's own Bash tool** in production
([anthropics/claude-code#14465](https://github.com/anthropics/claude-code/issues/14465)) — worked
in SDK 1.0.119, broke in 2.0.72.

**MCP Inspector is a weaker validator** than production clients — it silently accepts invalid
JSON Schema constructs that Claude Code rejects
([inspector#1005](https://github.com/modelcontextprotocol/inspector/issues/1005)). Passing
Inspector proves nothing.

→ FR-3 gains axis 2 (presence, not just shape).

## 6. ⚠ Two HIGH security defects

**6a. Unsanitized upstream metadata on `tools/list`.** `handlers_tools.rs:288` and
`peer_contract.rs:290` push the raw upstream `Tool` — description, `inputSchema`,
`outputSchema` — straight to clients. `cached_upstream_tool`
(`upstream/pool/helpers.rs:420-448`) stores them verbatim.
`sanitize_tool_text`/`sanitize_schema` (`projection.rs:57-155`, which strip control/bidi
characters, injection markers like `<system>`/`[INST]`, and secret patterns) run **only** on
the Code Mode catalog path (`search.rs:135-141`). A malicious upstream can inject into any
direct client's context via schema `description` at any depth. The original CONTRACT §C5/C8
would have codified this verbatim relay as a MUST without a carve-out.

**6b. `$ref` expansion DoS.** `schema_to_type` (`ts_signatures.rs:77-202`) caps *depth* at 20
and removes `seen_refs` on return — correct for accuracy, but no memoization. Non-cyclic shared
`$ref` reuse gives O(B^depth): B=3 → ~3.5e9 expansions from a few-KB schema, far under
`MAX_SCHEMA_BYTES = 524_288`. Hangs/OOMs the shared catalog render path, which rebuilds on any
upstream schema change. The SPEC *asked* this question; §5.1 answered a different one (byte
size ≠ algorithmic complexity).

Lower severity: JSDoc escaping (`escape_jsdoc`, `ts_signatures.rs:432`) is correct but its
safety is *inherited* from 6a's sanitizer and untested standalone; the mixed-content unwrap
fallback serializes `_meta` into the sandbox (upstream's own, no Labby secret found).

→ SPEC FR-9a/FR-9b.

## 7. ⚠ FR-6 and FR-7 were mis-stated

**FR-6.** "Byte-identical" is false for error results:
`enrich_completed_tool_error_result` (`upstream/tool_error.rs:148-180`) deliberately rewraps
`structuredContent` as `{"error": …, "upstream_structured_content": <original>}`. Introduced by
`3e5ab3df` and **schema-locked** (`docs/contracts/schemas/agent-error.schema.json`, drift-tested).
Scope FR-6 to the success path. Also: a byte-identical test asserts the *absence* of
transformation — it proves nothing is filtered, so the redaction non-goal must be explicit.

**FR-7.** The error trace is `isError: true`, which D1 holds exempt — so there is **no
conformance violation to fix**, and as originally written FR-7 contradicted D1. Keep the
one-line fix; restate the reason as internal consistency for trace consumers.

## 8. ⚠ Truncation: wrong path documented

Two markers exist:

| Path | Shape | Default |
|---|---|---|
| `truncate.rs:178-192` | object with `truncated: true`, `next_action` — branchable | **yes, always-on** |
| `shape.rs:99-135` | prose string | no (`Off` is `#[default]`) |

The plan cited only `shape.rs`. AC-8 could have passed while testing a path most deployments
never take. Also: `result_shaping` is only inserted when policy ≠ `Off`
(`execute.rs:112-116`), so agents branching on it read `undefined` by default, and the trace
schema declares `result_shaping: {type: object}` with **no properties**, so the discriminator
is undiscoverable from `outputSchema`.

## 9. ⚠ NG-1's mitigation is hollow

`{"action":"schema","params":{"action":"gateway.list"}}` returns
`ActionSpec.returns` — a `&'static str` documented at `labby-primitives/src/action.rs:42-44`
as *"Type-name hint … Not a runtime contract — purely informational."* Live values:
`"DoctorReport"`, `"Catalog"`, `"stream<Finding>"` — labels resolving to nothing.

**No path exists today by which an agent learns any action's result shape.** Fine as scoping;
not fine as a normative SHOULD in a contract.

Related: `unknown` does **not** "force a narrowing check" — the sandbox is untyped QuickJS and
the `.d.ts` is never compiled. And `codemode.search` results omit `dts`/`output_schema`
entirely (`preamble.rs:227-237`); `describe` fetches types lazily and **fails open silently**
(`preamble.rs:389-401`), so a degraded result looks complete.

## 10. ✓ Confirmations

**rmcp API — all 9 claims verified.** `with_raw_output_schema` (`tool.rs:210-213`),
`output_schema: Option<Arc<JsonObject>>` (`:28-30`, camelCase), `JsonObject =
serde_json::Map<String, Value>` (`model.rs:45`), `CallToolResult::structured` sets both fields
using `value.to_string()`, no validation anywhere in rmcp, no version gating,
`with_output_schema::<T>` behind `server`, 3.1.0 is current, no json!-literal helper.

**Git history.**

- `67a335ad` touched **only** the error path; the `Some(Ok(result))` success branch is
  byte-identical to its parent. **FR-6 is new work**, and no test asserts success-path fidelity.
- `3e5ab3df` (one hour earlier) created the error-wrapping contract — directly justifying the
  success-only decision as a dated consequence.
- `peer_contract.rs` created 2026-07-25 (`e617a22c3`) as a parallel re-implementation; **that
  same commit fixed one instance of this drift class** with the message *"otherwise a change to
  either drifts silently."*
- FR-7 dated exactly: `logs_count` required 2026-06-08 (`68cc3b8aa`); error path added
  2026-07-11 (`bf75ed4ac`) without it. Live 25 days.
- The unwrap is byte-identical to `977cb2166` (2026-05-31) across three refactors — "already
  correct, lock it" is historically supported.
- No reverted `outputSchema` attempts. Single-author repo; route review to Jacob.

**Institutional memory.** `lab-3qn` names "single-point-of-truth bypass" as a meta-pattern
across five subsystems; `lab-3qn.13` documents MCP registry vs HTTP router as "two independent
feature-gated service lists **with no divergence test**… silently exposes different
capabilities per transport with **no compile-time error**." `lab-3ef` supplies the
three-simultaneous-changes checklist pattern. No learning contradicts any plan decision.
Nothing on `outputSchema`/`structuredContent` — novel ground.

**Performance.** Plan is safe. Paging is count-based (`MCP_LIST_PAGE_SIZE = 100`), so no page
shrinkage. Added bytes ~3.1 KB (7 builtins) / ~438 B (Code Mode), flat at scale. `Arc` sharing
works as designed; no deep clone per tool. Builtins are absent from the Code Mode catalog, so
no cache invalidation and no extra `.d.ts` work. The `peer_contract.rs:193` unification saves
~260 bytes against a caller allocating megabytes — **justify it as drift-prevention, not
speed**.

Adjacent, pre-existing, out of scope: `descriptor_contract_hash` materializes every descriptor
3× per hash (2× per proxied call); `list_tools_impl` builds **11** `PeerContract`s per call,
each deep-cloning route-scope `BTreeSet`s (~2,200 allocations on a 200-upstream route);
`upstream_tool_last_error` runs an O(U) async lock loop per page for one tracing field.

## 11. ⚠ Package proportionality

The simplicity reviewer judged the package ~10:1 oversized against a ~150–250 line diff, and
flagged the acceptance criteria being restated in four documents — a drift risk of exactly the
kind this issue exists to eliminate.

**Resolution applied:** SPEC §6 is the single source for acceptance criteria; other documents
link rather than restate. Schemas reduced 4 → 2 (the error envelope duplicated the already-published
`docs/contracts/schemas/agent-error.schema.json`; the catalog descriptor duplicated a Rust
struct with no enforcing test). MODELS/TYPES retained — explicitly requested deliverables — but
corrected and trimmed.

## 12. Repo convention discovered (not from an agent)

`docs/contracts/schemas/*.schema.json` is an established, **code-enforced** convention:
`crates/labby-runtime/tests/agent_error_schema.rs:86` and
`crates/labby-codemode/tests/code_mode_error_schema.rs:70` read the schema files from disk and
assert required-field presence and enum membership — *"read as plain JSON data (no
schema-validation dependency)."*

This answers decision D10 with an existing pattern rather than an invention, and means the
published envelope schema belongs at `docs/contracts/schemas/dispatch-envelope.schema.json`
with `$id` `https://dinglebear.ai/schemas/labby/dispatch-envelope-v1.json`, paired with a
`docs/contracts/*.md` contract carrying YAML frontmatter.

→ SPEC NFR-10, AC-15.
