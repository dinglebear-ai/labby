# Implementation Plan — Tool Annotations

Issue [#212](https://github.com/dinglebear-ai/labby/issues/212) · Epic `lab-g1av5`
Branch `feat/tool-annotations-20260805` · Base `origin/main` @ `132448802`

All line anchors below were re-verified against this branch's tree. Where the
issue text disagrees (`helpers.rs:368`), the issue is stale.

## Phase map

| Phase | Bead | Deliverable | Blocked by |
|---|---|---|---|
| [1](#phase-1--annotation-policy-module) | `.1a` | `mcp/annotations.rs` + policy tests (unwired) | — |
| [2](#phase-2--shared-descriptor-builders) | `.1a` | `mcp/descriptors.rs`, still returning **un-annotated** `Tool` | 1 |
| [3](#phase-3--rewire-both-mirror-sites) | `.1a` | Both call sites use the builders | 2 |
| — | `.1a` | **Acceptance: contract hash UNCHANGED** | 3 |
| [4](#phase-4--tests-for-labby-owned-tools) | `.1b` | Switch annotations on (~6 lines) + tests | 3 |
| [5](#phase-5--upstream-passthrough-verification) | `lab-g1av5.2` | Passthrough tests + F9 gating tests | 4 |
| [6](#phase-6--documentation) | `lab-g1av5.3` | MCP.md, mcp/CLAUDE.md, gateway/CLAUDE.md | 4 |
| ~~7~~ | ~~`lab-g1av5.4`~~ | **CUT** — see [REVIEW_FINDINGS § 5](REVIEW_FINDINGS.md#5-scope-decisions) | — |

> **Revised after two review rounds.** Read [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md)
> first. Material changes: the work splits by *behavior change* into `.1a`/`.1b`;
> shared builders live in a new `mcp/descriptors.rs`, not `permanent_tools.rs`;
> four proposed tests were defective and one already exists; Phase 7 is cut; and
> F9 (SPEC § 7) gates `.1b`.

**Why split here.** Not by call site — landing the wire without the hash ships a
`tools/list_changed` desync. Split by behavior: `.1a` is a provably no-op refactor
(~350 LOC) that rebases cleanly against the five overlapping branches and whose
acceptance criterion is *the hash does not move*; `.1b` is the semantic flip
(~200 LOC, tiny diff, large blast radius) and a clean revert point if F9 forces
`mcp_app`/`doctor` to stay destructive. Total ≈ 600–650 LOC, within budget.

Phases 5 and 6 are independent and may run in parallel after 4.

---

## Phase 1 — Annotation policy module

**Create** `crates/labby/src/mcp/annotations.rs` with `AnnotationPolicy`,
`SERVICE_HINTS`, `service_policy`, `service_annotations`, and the four meta-tool
consts. Full source in [TYPES.md § T2–T4](TYPES.md#t2-annotationpolicy).

**Declare** it in `crates/labby/src/mcp.rs`, keeping alphabetical order — it goes
immediately after `pub(crate) mod agent_error;` (`mcp.rs:5`):

```rust
pub(crate) mod annotations;
```

No `mod.rs` (workspace lint `mod_module_files = "deny"`), no new dependency.

### Phase 1 tests (in `annotations.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_default_registry;

    /// SPEC R7: a read-only claim requires zero destructive actions. Adding a
    /// destructive action to fs/server_logs/lab_admin must fail here.
    #[test]
    fn read_only_services_have_no_destructive_actions() {
        let registry = build_default_registry();
        for svc in registry.services() {
            let policy = service_policy(svc);
            if policy.read_only {
                assert!(
                    !svc.actions.iter().any(|a| a.destructive),
                    "service `{}` claims readOnlyHint but has destructive actions",
                    svc.name,
                );
                assert!(!policy.destructive, "service `{}`: readOnly implies !destructive", svc.name);
            }
        }
    }

    /// SPEC § 6: derivation must reproduce the normative table.
    #[test]
    fn derived_destructive_matches_decision_table() {
        const EXPECTED: &[(&str, bool)] = &[
            ("fs", false), ("server_logs", false), ("lab_admin", false), ("doctor", false),
            ("snippets", true), ("setup", true), ("gateway", true),
        ];
        let registry = build_default_registry();
        for (name, want) in EXPECTED {
            let Some(svc) = registry.services().iter().find(|s| s.name == *name) else {
                continue; // service not compiled into this feature slice
            };
            assert_eq!(service_policy(svc).destructive, *want, "service `{name}`");
        }
    }

    /// SPEC R4 / contract C8: an unlisted service must over-warn, not under-warn.
    ///
    /// The first draft of this test compared `LEAST_SAFE` to its own literal and
    /// could only fail if someone edited that definition — it never called
    /// `service_annotations`, leaving the fail-closed path (the property the whole
    /// design rests on) completely unexercised. Call the real function.
    #[test]
    fn unlisted_service_falls_back_to_least_safe() {
        let fake = fake_unregistered_service("definitely-not-a-real-service");
        assert_eq!(service_annotations(&fake), least_safe());
    }

    /// Nothing may ship without a reviewed hint row. Both directions: a new
    /// service with no row would silently take LEAST_SAFE; a stale row for a
    /// renamed service would silently linger.
    #[test]
    fn service_hint_table_is_exhaustive_and_has_no_orphans() {
        let registry = build_docs_registry();
        for svc in registry.services() {
            assert!(
                service_hints(svc.name).is_some(),
                "service `{}` has no reviewed hint row",
                svc.name,
            );
        }
        for name in ALL_HINT_ROW_NAMES {
            assert!(
                registry.services().iter().any(|s| s.name == *name),
                "hint row `{name}` names no live service",
            );
        }
    }

    /// R7 checks `destructive` flags, but "read-only" is a stronger claim that
    /// `ActionSpec` cannot express: a *mutating but non-destructive* action added
    /// to a read-only service would pass every other test — exactly the `doctor`
    /// trap in SPEC § 5. Pin the action set so any addition forces a re-audit.
    #[test]
    fn read_only_services_have_a_pinned_action_set() {
        const PINNED: &[(&str, &[&str])] = &[
            ("fs", &["fs.list"]),
            ("lab_admin", &["onboarding.audit"]),
        ];
        let registry = build_docs_registry();
        for (service, expected) in PINNED {
            let Some(svc) = registry.services().iter().find(|s| s.name == *service) else {
                continue;
            };
            let actual: Vec<&str> = svc
                .actions
                .iter()
                .map(|a| a.name)
                .filter(|n| *n != "help" && *n != "schema")
                .collect();
            assert_eq!(
                actual, *expected,
                "`{service}` is annotated read-only; its action set changed — re-audit \
                 before updating this list",
            );
        }
    }

    /// SPEC R1: all four hints explicit, `title` unset.
    #[test]
    fn policy_sets_all_four_hints_and_no_title() {
        let a = AnnotationPolicy::new(true, false, true, false).to_annotations();
        assert_eq!(a.read_only_hint, Some(true));
        assert_eq!(a.destructive_hint, Some(false));
        assert_eq!(a.idempotent_hint, Some(true));
        assert_eq!(a.open_world_hint, Some(false));
        assert!(a.title.is_none());
    }
}
```

`build_default_registry` is the registry constructor referenced by the
onboarding checklist in the root `CLAUDE.md`; confirm its exact path in
`crates/labby/src/registry.rs` and adjust the import if it differs.

---

## Phase 2 — Shared descriptor builders

**Edit** `crates/labby/src/mcp/permanent_tools.rs`. It already owns
`code_mode_descriptor` (`:56`) and documents itself as the home for
product-level tool identity, so the remaining descriptors join it.

### 2a. Annotate `codemode`

`permanent_tools.rs:56-77` currently ends with `.with_raw_output_schema(...)`.
Chain the annotations on:

```rust
    Tool::new(
        CODE_MODE_TOOL_NAME,
        format!("{}\n\n{}", code_mode_description(upstreams), code_mode_app_text_note()),
        code_mode_execute_schema(),
    )
    .with_raw_output_schema(code_mode_trace_output_schema())
    .with_annotations(crate::mcp::annotations::CODE_MODE.to_annotations())
```

This one function already feeds both mirror sites (`handlers_tools.rs:166-169`
and `peer_contract.rs:220-227`), so `codemode` needs no further work.

### 2b. Builtin service descriptor

Add `builtin_service_descriptor` exactly as given in
[TYPES.md § T6](TYPES.md#t6-shared-descriptor-builders).

### 2c. Meta-tool descriptors

Each of these is currently constructed twice. Add one builder per tool, moving
the existing schema/description/meta calls verbatim so only the annotation is new:

```rust
#[cfg(feature = "gateway")]
pub(crate) fn code_mode_ui_descriptor(upstreams: &[CodeModeUpstreamDescription]) -> Tool {
    Tool::new(
        CODE_MODE_UI_TOOL_NAME,
        code_mode_ui_description(upstreams),
        code_mode_execute_schema(),
    )
    .with_raw_output_schema(code_mode_trace_output_schema())
    .with_meta(code_mode_tool_meta(CODE_MODE_UI_TOOL_NAME))
    .with_annotations(crate::mcp::annotations::CODE_MODE.to_annotations())
}

#[cfg(feature = "gateway")]
pub(crate) fn mcp_app_descriptor() -> Tool {
    Tool::new(MCP_APP_TOOL_NAME, mcp_app_tool_description(), mcp_app_tool_schema())
        .with_annotations(crate::mcp::annotations::MCP_APP.to_annotations())
}

#[cfg(feature = "gateway")]
pub(crate) fn add_server_descriptor() -> Tool {
    Tool::new(
        ADD_SERVER_TOOL_NAME,
        "Open a responsive form to test and add a remote or local MCP server to the Labby gateway catalog.",
        add_server_tool_schema(),
    )
    .with_meta(add_server_tool_meta(ADD_SERVER_TOOL_NAME))
    .with_annotations(crate::mcp::annotations::ADD_SERVER.to_annotations())
}

#[cfg(feature = "gateway")]
pub(crate) fn gateway_status_descriptor() -> Tool {
    Tool::new(
        GATEWAY_STATUS_TOOL_NAME,
        "Display live connection status, capabilities, and warnings for gateway upstream MCP servers.",
        gateway_status_tool_schema(),
    )
    .with_meta(gateway_status_tool_meta(GATEWAY_STATUS_TOOL_NAME))
    .with_annotations(crate::mcp::annotations::GATEWAY_STATUS.to_annotations())
}
```

The two description string literals move out of `handlers_tools.rs` — check that
`peer_contract.rs` used byte-identical strings before deleting its copies. If
they differ, that is a pre-existing latent hash bug worth calling out in the PR.

---

## Phase 3 — Rewire both mirror sites

### 3a. `crates/labby/src/mcp/handlers_tools.rs:138-145`

Before:

```rust
                    advertised_names.insert(svc.name.to_string());
                    let tool = Tool::new(svc.name, svc.description, Arc::clone(&schema));
                    let tool = if svc.name == SERVER_LOGS_TOOL_NAME && server_logs_app_visible {
                        tool.with_meta(server_logs_tool_meta(svc.name))
                    } else {
                        tool
                    };
                    tools.accept(tool);
```

After:

```rust
                    advertised_names.insert(svc.name.to_string());
                    tools.accept(permanent_tools::builtin_service_descriptor(
                        svc,
                        Arc::clone(&schema),
                        server_logs_app_visible,
                    ));
```

Then replace the four meta-tool constructions at `:199`, `:212`, `:225`, `:239`
with the Phase 2c builders, preserving the surrounding `tracing::info!` calls,
`advertised_names.insert(..)`, `gateway_tool_count += 1`, and every
`if !tools.finished()` guard. **Pagination correctness depends on those guards —
do not restructure the control flow.**

### 3b. `crates/labby/src/mcp/peer_contract.rs:204-211`

Before:

```rust
                let tool = Tool::new(service.name, service.description, Arc::clone(&schema));
                let tool =
                    if service.name == SERVER_LOGS_TOOL_NAME && self.audience.admin_apps_visible {
                        tool.with_meta(server_logs_tool_meta(service.name))
                    } else {
                        tool
                    };
```

After:

```rust
                let tool = permanent_tools::builtin_service_descriptor(
                    service,
                    Arc::clone(&schema),
                    self.audience.admin_apps_visible,
                );
```

Note the visibility flag is spelled differently at the two sites
(`server_logs_app_visible` vs `self.audience.admin_apps_visible`) but is
semantically the same gate — the shared builder takes it as a parameter rather
than recomputing it.

Then replace the meta-tool constructions at `:230`, `:243`, `:254`, `:266`.

**Invariant to preserve:** after this phase, `Tool::new` for a Labby-owned tool
appears **only** inside `permanent_tools.rs`. A grep for `Tool::new` in
`handlers_tools.rs` and `peer_contract.rs` should return nothing outside tests —
a cheap way to prove the mirror can no longer drift.

---

## Phase 4 — Tests for Labby-owned tools

Location: `crates/labby/src/mcp/handlers_tools/tests.rs` (`#[cfg(feature = "gateway")]`).
Template: `list_tools_advertises_code_mode_output_schemas` at `tests.rs:983-1043`.

> **The first draft of this test was vacuous.** `completion_test_registry()`
> (`tests.rs:133-155`) registers only `hidden-upstream` and `gateway-alpha`, and
> `code_mode_manager(true)` yields `RootSynthetic`, whose `hides_raw_tools()`
> (`catalog.rs:51-53`) suppresses every builtin — so at least 9 of the 12 rows hit
> `continue` and the test passed while proving almost nothing. Two fixes are
> mandatory: build the server so the builtins are actually visible, and **replace
> `continue` with coverage enforcement**.

```rust
#[tokio::test]
async fn list_tools_advertises_annotations_on_labby_owned_tools() {
    // build_docs_registry(): includes services whose runtime registration depends
    // on local operator config (lab_admin), so this does not silently skip.
    // code_mode_manager(false): keeps visibility Raw so builtins are advertised.
    let server = test_server(
        build_docs_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
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
    let result = running.service().list_tools_impl(None, context).await.expect("list tools");

    // (name, read_only, destructive, idempotent, open_world) — SPEC § 6.
    const EXPECTED: &[(&str, bool, bool, bool, bool)] = &[
        ("fs",             true,  false, true,  false),
        ("server_logs",    false, true,  false, false), // documented R2 override, SPEC § 6
        ("lab_admin",      true,  false, true,  false),
        ("gateway_status", true,  false, true,  false),
        ("doctor",         false, false, true,  true),
        ("mcp_app",        false, false, true,  false),
        ("setup",          false, true,  false, true),
        ("gateway",        false, true,  false, true),
        ("snippets",       false, true,  false, true),
        ("codemode",       false, true,  false, true),
        ("codemode_ui",    false, true,  false, true),
        ("add_server",     false, true,  false, true),
    ];

    // Coverage enforcement: every expected tool MUST be present. A `continue`
    // here is what made the first draft vacuous.
    for (name, ro, de, id, ow) in EXPECTED {
        let tool = result
            .tools
            .iter()
            .find(|t| t.name.as_ref() == *name)
            .unwrap_or_else(|| panic!("expected tool `{name}` was not advertised"));
        let a = tool.annotations.as_ref()
            .unwrap_or_else(|| panic!("tool `{name}` must carry annotations"));
        assert_eq!(a.read_only_hint,   Some(*ro), "{name}.readOnlyHint");
        assert_eq!(a.destructive_hint, Some(*de), "{name}.destructiveHint");
        assert_eq!(a.idempotent_hint,  Some(*id), "{name}.idempotentHint");
        assert_eq!(a.open_world_hint,  Some(*ow), "{name}.openWorldHint");
        assert!(a.title.is_none(), "{name}: title must stay unset");
    }
}
```

Add the inverse too: **every** Labby-owned tool in `result.tools` must have
`annotations.is_some()`, so a future tool added without a policy fails CI rather
than shipping bare.

Also required in this phase:

1. **Mirror equality (SPEC R6) — extend, do not add.** A mirror-equality test
   **already exists** at `tests.rs:1672-1683`, doing `assert_eq!(result.tools,
   contract_tools)` on the full `Vec<Tool>`. `Tool` derives `PartialEq`
   (`rmcp-3.1.0/src/model/tool.rs:13`), so the earlier "compare serialized JSON if
   `Tool` lacks `PartialEq`" hedge is dead — delete it. Extend that fixture to
   cover annotations. **Scope caveat:** R6 holds only below the 100-item page cap —
   `list_tools_impl` truncates, `visible_tool_descriptors` never paginates, and the
   multihop harness already uses 75 tools per leaf. Document the bound.
2. **Hash assertions — no magic constant.** A hard-coded pre-change baseline is a
   CI-churn generator: the hash covers the whole serialized `Tool`, so any
   description or schema edit flips it and the failure reads as two hex strings.
   Assert instead: (a) hashing the same descriptors twice in-process is equal;
   (b) hashing the set with vs without annotations differs. Both self-describing.
   Note `catalog.rs:526-548` already proves annotations move the hash.
3. **Regression sweep.** `tests.rs:852/893/915/950` (`*_tool_meta_*`),
   `tests.rs:1312`, `tests.rs:1634`, `tests.rs:1697`, and `tests.rs:2874`
   (pagination — count-based, so larger descriptors cannot shift page boundaries)
   must still pass untouched.
4. **Automate the mirror invariant.** "No `Tool::new` for a Labby-owned tool
   outside `descriptors.rs`" must become a CI check — clippy `disallowed_methods`,
   an `xtask` lint, or equivalent. As a one-time manual grep it will not survive
   the five in-flight branches touching these files.

---

## Phase 5 — Upstream passthrough verification

**No production change expected.** The full `Tool` is already moved downstream
verbatim: `helpers.rs:423` caches it into `UpstreamTool.tool`
(`upstream/types.rs:34`), `pool/tools.rs:67-128` clones it, and
`handlers_tools.rs:288` / `peer_contract.rs:290` accept it by move. If a test
proves otherwise, fix minimally and say so in the PR.

### 5a. Annotated fixture

Beside `fixture_upstream_tool` (`tests.rs:377-399`):

```rust
fn fixture_annotated_upstream_tool(upstream: &str, name: &str) -> UpstreamTool {
    let mut ut = fixture_upstream_tool(upstream, name, None);
    ut.tool.annotations = Some(rmcp::model::ToolAnnotations::from_raw(
        Some("Upstream Title".to_string()), // title must survive
        Some(true),                          // read_only
        None,                                // destructive absent — partial block
        Some(true),                          // idempotent
        Some(false),                         // open_world
    ));
    ut
}
```

Assert the downstream tool's annotations are **identical**, including the
preserved `title` and the still-absent `destructive_hint`. A partial block is
the strong test: it fails if Labby ever "helpfully" fills defaults.

### 5b. Caching preserves annotations

In `crates/labby-gateway/src/upstream/pool/helpers.rs` tests (beside `:584-631`):

```rust
#[test]
fn cached_upstream_tool_preserves_annotations_verbatim() {
    let annotations = ToolAnnotations::from_raw(Some("T".into()), Some(true), None, None, None);
    let tool = make_tool_with_annotations("t", annotations.clone());
    let (_name, cached) = cached_upstream_tool(tool, &Arc::from("up"));
    assert_eq!(cached.tool.annotations, Some(annotations));
    assert!(!cached.destructive, "read_only:true implies non-destructive");
}
```

The two existing fail-closed tests must remain **unmodified**.

### 5c. Subject-scoped OAuth path

`pool/tools.rs:246-274` returns raw `Vec<(String, Vec<Tool>)>`, accepted at
`handlers_tools.rs:311-318` — a second path that bypasses `UpstreamTool`
entirely. Cover it explicitly; it is the most likely place for a future
regression.

### 5d. Multihop

`crates/labby/examples/mcp_multihop_conformance.rs`: give `leaf_tool`
(`:97-106`) annotations, and assert at the root (`:851-880`) that they survive
two hops byte-identical.

**Do not** attempt the `hop2_destructive` assertions here — they are **not
implementable** in this harness. `mcp_multihop_conformance` is an out-of-process
driver (`scripts/ci/mcp-conformance.sh:192-198`) and `UpstreamTool.destructive`
lives in `labby-gateway/src/upstream/types.rs`, never crossing the wire. Assert
only what *is* observable out-of-process: byte-identical annotation survival
across two hops.

### 5e. F9 gating tests (in-process) — **regression guard**

F9 is resolved and accepted (SPEC § 7): every caller in this deployment has
`can_execute() == true`, so the widened reach is inert today. These tests still
ship, to pin that fact — the day an OAuth client is issued a scope set lacking
both `lab` and `lab:admin`, or `static_token_scopes` is narrowed, they fail
instead of quietly widening access.

Build the caller with `can_execute: false` explicitly (do not rely on a scope
string): `CodeModeCaller::Scoped { capabilities: CodeModeCallerCapabilities {
can_execute: false, .. }, .. }`. Assert against `cached_upstream_tool` +
`code_mode_host`:

```rust
// UpstreamTool.destructive gates more than MRTR: a hard `forbidden` in
// code_mode_host.rs:90-107 and palette.rs:235-247, where
// destructive_permitted(Mcp, c) == c.can_execute().
assert!(!hop2_destructive("fs"),          "annotated read-only tool widens next-hop reach");
assert!( hop2_destructive("setup"),       "destructive-annotated tool stays gated");
assert!( hop2_destructive("server_logs"), "R2 override keeps it gated (SPEC § 6)");
assert!( hop2_destructive("legacy_tool"), "unannotated upstream still fails closed");
```

Then make the accept/reject call on F9 explicitly. If the widened reach is
unacceptable, amend one cell: keep `mcp_app` and `doctor` at
`destructiveHint: true`.

---

## Phase 6 — Documentation

| File | Change |
|---|---|
| `docs/surfaces/MCP.md` | New "Tool annotations" subsection after "Destructive Actions" (`:64-68`): decision table, union caveat, verbatim passthrough, one-time hash churn, rmcp hints-not-guarantees caveat. |
| `crates/labby/src/mcp/CLAUDE.md` | Mirror invariant (descriptors only via `permanent_tools.rs`); union semantics; maintenance rule for new services. |
| `crates/labby-gateway/src/gateway/CLAUDE.md:95-96` | Disambiguate "annotation" — that sentence means `ActionSpec.destructive`, not `ToolAnnotations`. |

**Wording gate.** The phrase *"advisory only"* must not appear. Use: *"advisory
to clients; additionally consumed by Labby's own fail-closed derivation at the
next hop."* See [CONTRACT.md § C5](CONTRACT.md#c5-hints-are-advisory-to-clients--and-consumed-by-labby-at-the-next-hop).

Link this package from `docs/README.md`. Never hand-edit `docs/generated/**` —
run `just docs-generate`.

---

## ~~Phase 7 — Stretch: Code Mode hints~~ (CUT)

**Cut after review.** It carries the package's only real cache stampede:
`CatalogRenderCache` is a single `Arc<Mutex<Option<..>>>` slot with **no
single-flight** (`gateway/manager.rs:134`), and `CatalogEmbeddingCache` shares its
fingerprint (`code_mode.rs:130-138`) with a miss path that issues batched **TEI
network** calls. Changing `tool_shape_digest` invalidates every fingerprint at
once, so the first burst of Code Mode activity after upgrade produces N concurrent
full catalog renders plus N TEI round-trips — and `describe()` calls `list_tools()`
again per invocation, so N is not small.

Counter-argument on record: `codemode`'s `destructiveHint: true` conveys nothing
to a spec-compliant client (rmcp's documented default for that field is already
`true`), triggers no gating (`tool_request_is_destructive` returns false for
`codemode`), and has no finer-grained escape hatch — the Code Mode projection
discards both `tool.destructive` and `tool.tool.annotations`
(`gateway/code_mode/search.rs:123-142`). That is a real gap; it is recorded in
SPEC § 10.3 rather than closed here.

Prerequisite if ever resurrected: single-flight on `CatalogRenderCache` /
`CatalogEmbeddingCache`, which deserves its own bead on independent merit.

---

## Verification

```bash
just lint && cargo nextest run --workspace --all-features && just docs-check
```

Per-phase quick loop:

```bash
cargo nextest run -p labby --all-features
```

Full gate before review:

| Check | Command |
|---|---|
| Format + clippy + drift | `just lint` |
| Workspace tests | `cargo nextest run --workspace --all-features` |
| Feature slices | `cargo check -p labby --no-default-features --features gateway` |
| Generated docs | `just docs-generate && just docs-check` |
| MCP conformance | `scripts/ci/mcp-conformance.sh` |

## Risks

| ID | Risk | Mitigation |
|---|---|---|
| R1 | A wrong `readOnly` silently **widens next-hop authorization** (F9), not just elicitation. | Per-action audit (SPEC § 5) + R7 invariant test + the pinned action-set test (Phase 1) + F9 gating tests (5e). Highest-severity risk. |
| R2 | Mirror sites drift, breaking `tools/list_changed`. | Single construction path + the existing equality test at `tests.rs:1672-1683` + an **automated** `Tool::new` lint (not a manual grep). |
| R3 | Confirmation fatigue on mixed tools — ~72% over-warn aggregate (`gateway` 12/64). | Inherent at tool granularity; documented in SPEC § 5 and CONTRACT C3. Spec default for absent `destructiveHint` is already `true`, so the real-world delta is small. |
| ~~R4~~ | ~~`tools/list_changed` storm on upgrade.~~ | **Withdrawn — cannot happen.** Peers seed `last_contract` at registration and a binary change implies a restart. See SPEC § 10.1. |
| R7 | `server_logs` exposes admin-gated, incompletely-redacted log content once its next-hop gate is removed. | Mitigated by the R2 override in SPEC § 6 (`destructiveHint: true`). Widening redaction beyond key matching is a parallel follow-up. |
| R8 | `doctor.proxy.check` becomes reachable as an SSRF probe with no confirmation anywhere in a chain. | Companion bead hardening `dispatch/doctor/params.rs` onto `labby_primitives::ssrf`; land no later than `.2`. |
| R5 | Conflicts with in-flight branches. | `audit/mcp-2026-07-28-capabilities`, `fix/pool-bulkhead-coverage-20260805`, `integration/sync-all-20260802`, `feat/google-credential-broker`, and `preserve/mcp-discovery-deploy-clean-20260802` all touch `helpers.rs`, `peer_contract.rs`, `catalog.rs`, or `handlers_tools/tests.rs`. Rebase early; land Phase 1–4 before they grow. |
| R6 | A future rmcp bump adds a field and re-churns the hash. | Contained by the exact `=3.1.0` pin. |
