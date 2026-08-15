# IMPLEMENTATION PLAN — issue #210

Executable plan. Every file:line verified against `feat/mcp-output-schema-210` @ `132448802`.
**Revised 2026-08-05** after the 10-agent research pass — see [RESEARCH.md](RESEARCH.md).

Read [SPEC.md](SPEC.md) and [CONTRACT.md](CONTRACT.md) first; their decisions are not re-argued
here. Acceptance criteria live in SPEC §6.

| Bead | Scope | Blocked by |
|---|---|---|
| `lab-41e7m.1` | §2 audit, §3 schema + registry builder + FR-7 | — |
| `lab-41e7m.2` | §4 unwrap contract, truncation, success-path proxy fidelity | — |
| `lab-41e7m.3` | §5 catalog coverage | **FR-9b** (see §7.2) |
| `lab-41e7m.4` | §6 docs | .1, .2, .3 |
| **new** | §7 security: FR-9a sanitization, FR-9b `$ref` DoS (blocks `.3`) | — |
| **new** | FR-2a authorization-gate consolidation — SPEC FR-2a | — |

**Hard constraint:** `code_mode_trace_output_schema` MUST remain in `handlers_tools.rs` for this
epic. `.2` asserts against it while `.1` refactors that file; moving it breaks `.2` mid-flight.

**Crate overlap:** `.2` and `.3` touch disjoint *files* but overlapping *crates*
(`labby-codemode`, `labby-gateway`). Any shared `Cargo.toml` edit needs coordination.

---

## 1. Environment

```bash
cd /home/jmagar/workspace/labby/.worktrees/feat-mcp-output-schema-210
```

```bash
cargo nextest run -p labby --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
```

> `target/`, `dist/`, `apps/palette-tauri/*` show as untracked — the sync engine symlinked them
> and `.gitignore` uses directory patterns. Keep commits path-limited; never `git add -A`.

---

## 2. Step 1 — Audit (before any schema is attached)

Advertising `outputSchema` converts best-effort structured content into a client-enforced
requirement. **The Python SDK hard-errors** if a declaring tool returns no `structuredContent`,
and that exact failure already broke Claude Code's own Bash tool
([#14465](https://github.com/anthropics/claude-code/issues/14465)).

Audit **two axes** per tool:

1. **Shape** — does every success path produce the C2.1 envelope?
2. **Presence** — does every success path set `structuredContent` at all?

```bash
rg -n 'reg\.register\(' crates/labby/src/registry.rs
rg -n 'CallToolResult::(success|error|structured)' crates/labby/src/mcp/
ls crates/labby/src/mcp/services/          # MCP-specific exception paths
```

Record on `lab-41e7m.1`:

| Tool | Class | Success shape | Structured always set? | Site | Schema |
|---|---|---|---|---|---|
| `gateway` `doctor` `setup` `server_logs` `snippets` `fs` `lab_admin` | builtin | envelope (expected) | **verify** | `result_format.rs:111-114` | envelope |
| `codemode` / `codemode_ui` | Code Mode | trace | yes | `call_tool_codemode.rs:803` | trace *(already)* |
| `mcp_app` | synthetic | **TBD** | **TBD** | `call_tool.rs:273+` | TBD |
| `add_server` | synthetic | **TBD** | **TBD** | `call_tool.rs:459+`, `:1019` | TBD |
| `gateway_status` | synthetic | **TBD** | **TBD** | `call_tool.rs:601+` | TBD |
| upstream `*` | proxied | upstream's own | n/a | relayed | untouched |

Rules:

- Envelope schema **only** where both axes pass. When ambiguous, advertise **nothing** — an
  absent schema is always conformant.
- `status == "stub"` services (`registry.rs:60-65`) can only return `unknown_action` errors.
  Advertising a success schema is harmless but pointless; record the decision.
- There is **no** `lab.help` global tool — `help` is a built-in action plus `lab://catalog`.

---

## 3. Step 2 — Schema, builder, gating (`lab-41e7m.1`)

### 3.1 The schema function

Mirror `code_mode_trace_output_schema` (`handlers_tools.rs:686-765`).

```rust
/// Success-envelope output schema shared by every builtin service tool.
///
/// Mirrors `build_success` (mcp/envelope.rs:42-49). `data` is intentionally
/// unconstrained: one tool serves many actions, so a tool-level schema cannot
/// describe per-action payloads.
///
/// Error envelopes are deliberately NOT described here — see
/// docs/plans/complete/210-mcp-output-schema/CONTRACT.md §C3.2. Note the exemption for
/// `isError` results is ecosystem convention, not explicit spec text.
///
/// `additionalProperties` is `true` by decision (SPEC §5.2). If `build_success`
/// ever grows a field, this schema changes in the SAME commit anyway
/// (CONTRACT §C3.5) — the open object just means clients do not break first.
pub(crate) fn dispatch_envelope_output_schema() -> Arc<serde_json::Map<String, Value>> {
    static ENVELOPE_OUTPUT_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> =
        LazyLock::new(|| match serde_json::json!({
            "type": "object",
            "properties": {
                "ok": { "const": true },
                "service": { "type": "string",
                    "description": "Service tool that answered the call." },
                "action": { "type": "string",
                    "description": "Resolved dotted action, including the built-in `help` and `schema` actions." },
                "data": { "description": "Action-specific payload; shape varies by action." }
            },
            "required": ["ok", "service", "action", "data"],
            // `true`, deliberately (SPEC §5.2): closing the envelope would make
            // any future `build_success` field break all seven builtins'
            // advertised schemas at once, client-side, AND move the contract
            // hash. AC-3's "exactly four keys" test gives the same detectability
            // internally without exporting the brittleness.
            "additionalProperties": true
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("dispatch envelope output schema must be an object"),
        });
    Arc::clone(&ENVELOPE_OUTPUT_SCHEMA)
}
```

Add a comment at `envelope.rs:42` binding `build_success` and this schema to the same commit.

### 3.2 Extend the existing builder — do NOT create a new module

**`permanent_tools.rs` is where the pattern is precedented — once.** State this precisely:
`PermanentToolRegistry` is a unit struct (`:38`) fronting `const PERMANENT_TOOLS:
[PermanentToolEntry; 1]` (`:32-35`) whose job is `resolve(name) -> PermanentToolId`. Builtin
service tools come from `ToolRegistry::services()` and will never appear in `PERMANENT_TOOLS`.
What *is* true: `code_mode_descriptor` (`:56-77`) is a registry-owned constructor calling
`Tool::new(...).with_raw_output_schema(...)`, reached from both call sites via
`self.registry.permanent_tools()` (`handlers_tools.rs:166-169`, `peer_contract.rs:222-225`).

Add the new constructors there rather than creating a parallel `tool_descriptors.rs` — but
**in the same commit, rewrite the module doc** (currently "Product-level MCP tools whose identity
and dispatch exist independently of upstream health") to name both responsibilities: permanent
dispatch identity, *and* the sole construction site for Labby-owned `Tool` descriptors. Leaving
that doc stale is how the next reader re-invents `tool_descriptors.rs`. (Alternative, equally
valid: create the descriptor module and move `code_mode_descriptor` into it. Ship one, not both.)

```rust
impl PermanentToolRegistry {
    /// Descriptor for one builtin service tool.
    ///
    /// The `SERVER_LOGS_TOOL_NAME` check is invariant across callers and lives
    /// here; only `admin_apps_visible` differs, because the live-request path
    /// resolves it from request auth while the stored peer contract resolves it
    /// from a captured `PeerCatalogAudience`.
    pub(crate) fn builtin_service_tool(
        &self,
        service: &RegisteredService,
        admin_apps_visible: bool,
    ) -> Tool {
        let tool = Tool::new(service.name, service.description, builtin_action_schema())
            .with_raw_output_schema(dispatch_envelope_output_schema());
        if service.name == SERVER_LOGS_TOOL_NAME && admin_apps_visible {
            tool.with_meta(server_logs_tool_meta(service.name))
        } else {
            tool
        }
    }
}
```

Add one method per synthetic tool the audit clears (`mcp_app_tool`, `add_server_tool`,
`gateway_status_tool`, `code_mode_ui_tool`). Do this **even for tools that get no output
schema** — `handlers_tools.rs:227` ≡ `peer_contract.rs:256` and `:241` ≡ `:268` are
character-for-character duplicated description literals, live drift surface today.

`builtin_action_schema()` moves the `LazyLock` static (`handlers_tools.rs:48-49`) into the
registry so `peer_contract.rs:193`'s per-call `Arc::new(action_schema())` is retired.
**Justify this as single-definition/drift-prevention, not performance** — it saves ~260 bytes
against a caller that allocates megabytes.

**Encapsulation.** Keep `dispatch_envelope_output_schema` and `builtin_action_schema` private to
the registry module; export only assembled `Tool`-returning constructors, so bypass requires
deliberately reaching past the boundary. Consider a `clippy.toml` `disallowed_methods` entry for
direct `Tool::new` outside the module (the repo already uses `disallowed_macros` for
`#[async_trait]`).

### 3.3 Rewire both call sites

`handlers_tools.rs:139-144`:

```rust
tools.accept(
    self.registry
        .permanent_tools()
        .builtin_service_tool(svc, server_logs_app_visible),
);
```

`peer_contract.rs:204-212`: same call with `self.audience.admin_apps_visible`, pushing to
`descriptors`.

**Preserve exactly:** pagination gating (`tools.finished()` at `:147` and every later guard),
`advertised_names`/`builtin_names` bookkeeping, the `hide_raw_tools` skip (`:135`), counters,
and existing tracing. Pure extraction — any behavior change is a bug.

Two cleanups that are safe to fold in because they are provably no-ops:

- `handlers_tools.rs:131` — `route_scope.allows_service(...)` is redundant;
  `service_visible_on_mcp` already checks it (`peer_contract.rs:105`).
- `builtin_names` is `Vec<&str>` here vs `HashSet<String>` in `peer_contract.rs:195`; align.

**Do NOT fold in** the `PeerContract` hoist (11 constructions per `list_tools_impl`) — real, but
it changes allocation behavior and deserves its own test. SPEC FU-2.

### 3.4 FR-2a — MOVED OUT of this bead

Consolidating `add_server_app_available*` / `gateway_status_app_available*` /
`action_allowed_on_mcp` is now **its own bead**. It has 8 call sites, 6 of them outside `.1`'s
files (`call_tool.rs:461`, `:494`, `:603`, `:679`; `handlers_resources.rs:408`, `:434`, `:1071`,
`:1170`), and it changes authorization rather than descriptor shape — a different blast radius
and a different revert unit.

Read SPEC FR-2a before starting it. The three constraints that must survive into that bead:
the consolidated gate stays **audience-free** (folding `audience.admin_apps_visible` inward
silently grants admin apps to unprivileged callers, because `catalog.rs:176-185` supplies
`PeerCatalogAudience::default()` with `admin_apps_visible: true`); it is shaped as a **free
function**, not a `PeerContract` method reached via `self.peer_contract()` (which would put a
deep `McpRouteScope` clone on every builtin dispatch); and it needs a test asserting non-admin
**denial** at the dispatch and resource paths, not just at `tools/list`.

### 3.5 FR-7 — reconcile the error trace

Add `logs_count: 0` at `call_tool_codemode.rs:659-670`, alongside the existing `"output_tokens": 0`
placeholder:

```rust
    "result_shape": { "type": "undefined" },
    "logs_count": 0,
});
```

**State the reason correctly in the commit message:** internal consistency for trace consumers
(the inline inspector reads `structuredContent` on both paths). It is **not** a conformance fix —
the trace is `isError: true` and therefore exempt under CONTRACT §C3.2. Dated: `logs_count`
required since 2026-06-08 (`68cc3b8aa`); this error path added 2026-07-11 (`bf75ed4ac`).

### 3.6 Tests

**Extend the existing drift test — do not write a weaker one.**
`handlers_tools/tests.rs:1671-1684` already asserts full structural `Tool` equality:

```rust
assert_eq!(
    result.tools, contract_tools,
    "tools/list and the notification contract must use identical descriptors"
);
```

That covers `_meta`, annotations, title, icons — strictly more than a field-by-field
comparison. **Its gap is coverage**: it runs only with `code_mode_manager_with_pool(true, ...)`,
where `hide_raw_tools` suppresses every builtin but `server_logs`, so it never exercises the
loop being changed.

Add a Raw-mode sibling, following the real fixture pattern verbatim:

```rust
#[tokio::test]
async fn raw_mode_builtin_descriptors_match_across_builders() {
    // HERMETIC: force Raw unconditionally. `Root` + `gateway_manager: None` is
    // NOT sufficient — `peer_contract.rs:89-91` returns InProcessPeer when
    // `gateway_manager.is_none() && config::process_code_mode_enabled()`, and
    // that backing store is a process-global `AtomicBool` (`config.rs:64-66`)
    // any other test in the binary can set. `expose_code_mode: false` forces
    // Raw at `peer_contract.rs:78`. The repo solves this hazard the same way —
    // see `peers.rs:186-188`.
    //
    // The services list MUST be populated: `service_visible_on_mcp` gates on
    // `route_scope.allows_service` (`peer_contract.rs:105`), so an empty list
    // advertises nothing and the test proves nothing again.
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "raw-mode-drift",
        [],
        ["hidden-upstream", "gateway-alpha", "danger"],
        /* expose_code_mode */ false,
    );
    let server = test_server(
        completion_test_registry(),
        None,
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");

    assert_eq!(
        result.tools, contract_tools,
        "raw-mode tools/list and the notification contract must use identical descriptors"
    );

    // `completion_test_registry()` registers `hidden-upstream`, `gateway-alpha`,
    // and `danger` — there is NO service named `gateway`. Asserting a name the
    // fixture lacks would make this test fail; asserting nothing would let it
    // pass vacuously, which is the existing test's failure mode.
    let names = result.tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>();
    assert!(names.contains(&"gateway-alpha"), "raw mode must advertise builtins");
    assert!(
        names.contains(&"hidden-upstream"),
        "raw mode must NOT suppress builtins — the code-mode sibling asserts the inverse"
    );

    // Scope the schema assertion to registry services. Synthetic tools may
    // legitimately carry no schema (`mcp_app` returns
    // `{"kind":"mcp_app_control", …}`, call_tool.rs:400-411), so a blanket loop
    // over `result.tools` would panic on a correctly schema-less tool the moment
    // this fixture gains a manager or an upstream.
    let service_names: Vec<&str> =
        completion_test_registry().services().iter().map(|s| s.name).collect();
    for tool in result.tools.iter().filter(|t| service_names.contains(&t.name.as_ref())) {
        let schema = tool.output_schema.as_ref()
            .unwrap_or_else(|| panic!("{} advertises no outputSchema", tool.name));
        assert_eq!(schema["properties"]["ok"]["const"], serde_json::json!(true));
    }
}
```

Both guards matter. Without the positive name assertions the test passes vacuously — the exact
failure mode of the existing code-mode test. Without scoping the schema loop it panics on
synthetic tools that are *correctly* schema-less.

Also required:

- **Snapshot parity** (AC-2a): `ToolCatalogSnapshot::from_descriptors(&result.tools)` equals
  `snapshot_tool_catalog_for_request(&ctx)`. `output_schema` is inside
  `descriptor_contract_hash` (`catalog.rs:125-143`), which drives `tools/list_changed` — one-sided
  drift makes change detection *wrong*, not merely incomplete.
- **Envelope conformance + presence**: call `help` on each builtin (it flows through
  `format_dispatch_result`); assert `structuredContent` is present, has exactly four keys, and
  that the text block parses to the identical value.
- **Error exemption**: unknown action ⇒ `is_error == Some(true)` and an `{ok: false}` envelope;
  do not apply the success schema to it.
- **Published-schema drift** (AC-15): follow `crates/labby-runtime/tests/agent_error_schema.rs` —
  read the `.json` as plain data, assert `required` fields and const/enum members. No validator
  dependency.
- **Regression**: `list_tools_advertises_code_mode_output_schemas` (`tests.rs:984`) and
  `code_mode_trace_output_schema_advertises_structured_trace_kinds` (`:972`) still pass.

---

## 4. Step 3 — Lock the Code Mode contract (`lab-41e7m.2`)

Tests and docs only. Byte-identical since 2026-05-31 across three refactors — if a test exposes
a bug, report on the bead before fixing.

### 4.1 Document the precedence

Expand the doc comment at `code_mode_host.rs:605` with the CONTRACT §C6 table, including: rule 1
precedes content inspection; `if let Some(..)` tests presence not truthiness; divergences from
Cloudflare (no `toolResult`; empty ⇒ `Null`).

### 4.2 Edge-case matrix

Falsy structured values (`false`, `0`, `null`, `""`); structured + text/resource both present
(structured wins, `_meta` ui link still captured); multiple text blocks (valid-after-join and
split-mid-token); empty content ⇒ `Null`; mixed content ⇒ raw result — **including an assertion
of what `_meta` is exposed** (CONTRACT §C6 rule 4); `isError` + structured ⇒ `CodeModeCallError`.

### 4.3 Truncation fidelity — both markers

| Path | Assert |
|---|---|
| `truncate.rs:178-192` (**default**) | object marker with `truncated: true` and `next_action`; structured `calls[]` retained |
| `shape.rs:99-135` (non-`Off` policy) | marker string; nothing else re-serialized |

The original plan cited only `shape.rs`, which is **not** the default path. Cover both.

### 4.4 Success-path proxy fidelity (FR-6)

Confirmed new work: `67a335ad` changed only the error branch; its `Some(Ok(result))` branch is
byte-identical to its parent, and `crates/labby/src/mcp/upstream/tests.rs` has no success-path
`structuredContent` test today (only `completed_error_retains_every_upstream_payload_channel`,
which exercises `isError` wrapping).

Assert a **successful** upstream result relays `structuredContent` byte-identically, and — in
the same test module — that an error result *is* rewrapped as
`{"error": …, "upstream_structured_content": <original>}`, so the deliberate asymmetry is
pinned rather than looking like a bug.

---

## 5. Step 4 — Catalog coverage (`lab-41e7m.3`)

### 5.1 Verify the existing path

Extend `crates/labby-codemode/src/tests_ids_schema.rs` and the tests in
`crates/labby-gateway/src/gateway/code_mode/search.rs` (**note the crate** — the original plan
implied `labby-codemode`):

- upstream tool with `output_schema` ⇒ descriptor carries it; `.d.ts` shows `Promise<T>`;
- without ⇒ `json_schema_to_type(None)` yields `unknown` (`ts_signatures.rs:77-83`); assert no
  fabricated type;
- malformed/oversized ⇒ `sanitize_schema` handles it without panic or leakage.

### 5.2 Snippets — document, don't fill

At `types.rs:186`:

```rust
// Deliberate: a snippet returns an arbitrary JavaScript value, so there is no
// honest output schema to publish. `dts` is likewise empty, so `describe`
// renders inputs with no type section. See SPEC.md FR-8.
output_schema: None,
```

### 5.3 OpenAPI — audit, then decide

```bash
rg -n 'responses|output_schema|ToolDescriptor::tool' crates/labby-openapi/src/ crates/labby-codemode/src/local_provider.rs
```

**Audit only — always defer the implementation to a follow-up bead**, regardless of LOC. The
earlier "fits in ~150 LOC ⇒ implement" trigger traded a *truthfulness* question for an *effort*
question and contradicted CONTRACT §C3.1 ("when ambiguous, advertise nothing"). OpenAPI documents
intent; runtimes drift from their specs. Attaching a derived schema is the one place in this epic
where a **wrong** schema — not merely a missing one — can be introduced, and it would fail
client-side on a path that previously returned `unknown`. Record findings on the bead and file
the follow-up.

### 5.4 Not doing

Builtin tools are not added to the Code Mode catalog here — that is SPEC FU-1, and it is the
change that would make FR-1 meaningful under Code Mode.

---

## 6. Step 5 — Docs (`lab-41e7m.4`)

1. **`docs/surfaces/MCP.md`** — envelope schema on builtins; **state plainly it is Raw-mode-only**
   (AC-16); error envelopes deliberately outside `outputSchema` with the convention-not-spec
   caveat; no version gating.
2. **`docs/dev/CODE_MODE.md`** — CONTRACT §C6 precedence; both truncation markers; outer-boundary-only
   truncation.
3. **`docs/dev/ERRORS.md`** — verify-only; no new kinds expected.
4. **Generated docs** — `just docs-generate` && `just docs-check`. **Expected no-op**: artifacts
   render from the registry action catalog (`docs/render.rs:18`), not from `rmcp::model::Tool`,
   so none contains `outputSchema`. `.1` can land with docs green.
5. **Do NOT document a one-time fanout — there isn't one.** An earlier draft asserted every
   peer's hash moves once on upgrade. `server.rs:531-541` seeds each subscription's baseline from
   what that peer actually received, and `PeerRegistry` is in-memory (`peers.rs:147`), so an
   upgrade restarts the process and peers re-register against the new set. Fanout on upgrade is
   **zero**. The only real case is Labby-proxying-Labby, which is the ordinary upstream-change
   path. Writing the original claim into permanent docs would be worse than writing nothing.
6. **Promote the contract**: move CONTRACT.md → `docs/contracts/mcp-tool-output.md` (keep the
   frontmatter) and the envelope schema → `docs/contracts/schemas/dispatch-envelope.schema.json`
   with `$id` `https://dinglebear.ai/schemas/labby/dispatch-envelope-v1.json`, matching the
   existing convention and its drift-test pattern.

---

## 7. Step 6 — Security (new beads)

Neither defect is created by this issue, but both sit in its blast radius and one would be
codified by its contract.

### 7.1 FR-9a — sanitize upstream metadata on the `tools/list` path (HIGH)

`handlers_tools.rs:288` and `peer_contract.rs:290` push raw upstream `Tool` values —
`description`, `inputSchema`, `outputSchema` — to clients. `cached_upstream_tool`
(`upstream/pool/helpers.rs:420-448`) stores them verbatim, and
`sanitize_tool_text`/`sanitize_schema` (`gateway/projection.rs:57-155`) run only on the Code
Mode catalog path.

**Do not simply call `sanitize_schema` on the relay path.** It is not schema-keyword-aware:
`recurse` (`projection.rs:127-144`) sends every JSON string through `sanitize_tool_text`, which
deletes injection markers, redacts secret-shaped values, and truncates at 2048 chars. Correct for
`description`/`title`; destructive for `enum`, `const`, `default`, `pattern`, `format`, `$ref`.
Corrupting those makes Labby advertise a schema that its own byte-identical relayed results
(FR-6/C5.2) then violate — breaking strict clients on *legitimate* upstream tools.

Requirements: sanitize documentation-bearing keys only; leave value/validation keywords untouched;
keep `sanitize_schema`'s 512 KB gate at render time rather than moving it to cache time
(`projection.rs:117-123` records that a 16 KB gate already collapsed legitimate cortex/axon
schemas to `unknown` and was reverted); ensure idempotency, since the Code Mode catalog path
already sanitizes. Prefer one chokepoint at `cached_upstream_tool` **only if** these hold.

Tests: (a) a schema whose `description` carries injection markers and bidi overrides is stripped;
(b) a schema whose `enum` value is shaped like a secret (`ghp_…`) survives **verbatim**; (c) a
`pattern` containing `###` is unchanged.

### 7.2 FR-9b — bound `$ref` expansion (HIGH)

`schema_to_type` (`ts_signatures.rs:77-202`) caps depth at 20 (`:90-91`) and removes `seen_refs`
on return (`:105`), so non-cyclic shared `$ref` reuse is O(B^depth) — ~3.5e9 expansions at B=3,
well under `MAX_SCHEMA_BYTES = 524_288`.

**Thread a budget bounding BOTH node count and accumulated output bytes**, return `unknown` on
exhaustion, and `tracing::warn!` naming upstream and tool. The byte bound is not optional: the
function returns a `String` built by concatenation, so the *result* at B=3/depth 20 is O(3²⁰)
bytes — a multi-gigabyte allocation and an OOM kill, not a slow request. Cost on the normal path
is one `usize` increment per node, negligible beside the `object.clone()` already performed per
composition node (`:118`).

**Do NOT memoize per `(ref, root)`.** Expansion depends on the current `seen_refs` set, not on
`(ref, root)`: a cached entry returns a cycle-truncated result where full expansion was correct,
and which occurrence populates the cache is traversal-order dependent, so an unrelated schema edit
silently flips types to `unknown`. It also misses the composition arms (`:127-144` recurses over
every `anyOf`/`oneOf`/`allOf` element with no `$ref` involved) and bounds no output size.

**Land this before `.3`.** The vulnerable path sits behind a single-slot cache
(`Arc<Mutex<Option<..>>>`, `manager.rs:133`) whose rebuild runs outside the lock (`search.rs:122`+),
so N concurrent sessions on a cache miss each run the expansion — and a hostile upstream forces
the miss on demand. `.3` adds tests that push upstream `output_schema` through
`generate_tool_types`.

Test with a wide-and-deep `$defs` graph; assert bounded node count and bounded rendered length,
not just bounded time.

---

## 8. Sequencing

```
lab-41e7m.1 ─┐
lab-41e7m.2 ─┼─→ lab-41e7m.4 (docs)
lab-41e7m.3 ─┘
security beads — independent
```

Within `.1`: **audit → schema → registry builder → rewire → gating consolidation → FR-7 → tests.**
No schema is attached before its tool clears the audit.

## 9. Definition of done

Tracked in [PROGRESS.md](PROGRESS.md) §3 against SPEC §6. Summary gate:

- [ ] Audit (both axes) recorded before any attachment
- [ ] Raw-mode drift test passes **and** proves builtins were advertised
- [ ] Snapshot/contract-hash parity asserted
- [ ] Gating booleans have one implementation
- [ ] Unwrap matrix, both truncation markers, success-path proxy fidelity green
- [ ] Catalog verification green; snippets comment landed; OpenAPI audit recorded
- [ ] `cargo nextest run --workspace --all-features` clean
- [ ] `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean
- [ ] `just docs-check` clean; docs state the Raw-mode-only scope
- [ ] Security beads filed (not necessarily landed with this epic)
