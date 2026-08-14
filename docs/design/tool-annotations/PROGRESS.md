---
title: "Tool Annotations Progress"
created: "2026-08-05"
updated: "2026-08-13"
---

# Progress — Tool Annotations

Working document. Update as work lands; it is not generated.

Issue [#212](https://github.com/dinglebear-ai/labby/issues/212) · Epic `lab-g1av5`
Branch `feat/tool-annotations-20260805` · Base `origin/main`
Last updated: 2026-08-13 — **implementation and focused verification complete**

Legend: ☐ not started · ◐ in progress · ☑ done · ⊘ cut

## Status

| Bead | Title | Priority | Status |
|---|---|---|---|
| `lab-g1av5` | Epic: annotations + passthrough verification | P2 | ◐ implemented; final CI pending |
| `.1a` | No-op refactor: policy module + descriptors, unwired | P2 | ☐ *(to create — split from `.1`)* |
| `.1b` | Semantic flip: switch annotations on | P2 | ☐ *(to create — **unblocked**)* |
| `lab-g1av5.2` | Verify upstream passthrough + 5e regression guard | P2 | ☐ blocked by `.1b` |
| `lab-g1av5.3` | Document semantics, gating effect, maintenance rules | P2 | ☐ blocked by `.1b` |
| `lab-g1av5.4` | Stretch: Code Mode catalog hints | P3 | ⊘ **cut** — cache stampede; see REVIEW_FINDINGS § 5 |
| *new* | Companion: harden `doctor` SSRF validator | P2 | ☐ *(to create — land no later than `.2`)* |

## F9 — resolved, accepted (2026-08-05)

**Decision: Option A — annotate all five, no amendment. `.1b` is unblocked.**

The Code Mode gate is default-deny on the *absence* of both `lab` and
`lab:admin` (`call_tool_codemode.rs:807-815`) — not keyed on `lab:read`, which
alone does **not** satisfy it. Operator confirmed no OAuth client holds a scope
set lacking those two. The other paths were already safe: no-auth/stdio →
`TrustedLocal` (`:542`); default static-bearer scopes are
`["lab:read","lab:admin"]` (`labby-auth/src/config.rs:242`). So `can_execute()`
is true for every real caller and the widened reach is inert here.

**Conditional on a deployment fact, not an invariant.** `palette.rs:186-200`
already models narrower callers (`scoped_read_only { can_execute: false }`,
`gateway:<upstream>` scopes), and both `static_token_scopes` and the OAuth
`default_scope` are operator-configurable. Phase 5e ships as a **regression
guard** so a future narrow-scoped client fails CI rather than silently gaining
reach.

## Phase checklist

### `.1a` — provably no-op refactor (acceptance: **contract hash unchanged**)
- ☐ `crates/labby/src/mcp/annotations.rs` — free `to_annotations` fn + `match svc.name` + `least_safe()`
- ☐ `crates/labby/src/mcp/descriptors.rs` — six builders, still returning un-annotated `Tool`
- ☐ Two `mod` lines in `crates/labby/src/mcp.rs`
- ☐ `permanent_tools::code_mode_descriptor` delegates to `descriptors::code_mode`
- ☐ Rewire `handlers_tools.rs` (`:139`, `:199`, `:212`, `:225`, `:239`)
- ☐ Rewire `peer_contract.rs` (`:204`, `:230`, `:243`, `:254`, `:266`)
- ☐ Pagination guards, `tracing` calls, `advertised_names.insert`, counters preserved verbatim
- ☐ Automated lint: no `Tool::new` for a Labby-owned tool outside `descriptors.rs`
- ☐ **Verify the contract hash did not move**

### `.1b` — semantic flip
- ☑ Chain `.with_annotations(..)` inside the centralized registry builders
- ☐ Gate the four meta-tool policies with `#[cfg(feature = "gateway")]`
- ☑ Test: 13 owned tools covered across the registry/meta descriptor tests
- ☑ Test: every Labby-owned tool has `annotations.is_some()`
- ☐ Test: read-only services have zero destructive actions (R7)
- ☐ Test: pinned action set for read-only services
- ☑ Test: hint table covers registry services and synthetic tools
- ☑ Test: unlisted service falls back to least-safe hints
- ☐ Test: hash stable across two in-process builds; differs with annotations stripped
- ☑ Both wire and peer-contract listing paths consume the same registry builders
- ☐ Regression sweep: `tests.rs:852/893/915/950/1312/1634/1697/2874`

### `lab-g1av5.2` — passthrough + gating
- ☐ `fixture_annotated_upstream_tool` (`&Arc<str>`, partial block, `title` set)
- ☑ Single-hop raw-listing passthrough assertion
- ☐ `cached_upstream_tool` preserves annotations (keep only the annotations half)
- ☐ Existing fail-closed tests pass **unmodified**
- ☐ Subject-scoped OAuth path covered
- ☐ Multihop: annotation survival only (**not** `hop2_destructive` — unimplementable there)
- ☐ **5e: in-process gating tests (regression guard)**

### `lab-g1av5.3` — docs
- ☑ `docs/surfaces/MCP.md` annotations subsection
- ☐ Fix `docs/surfaces/MCP.md:66-68` — it contradicts the code today
- ☑ `crates/labby/src/mcp/CLAUDE.md` mirror invariant + maintenance rule
- ☑ `crates/labby-gateway/src/gateway/CLAUDE.md` wording reconciled with current semantics
- ☐ Note: `lab://<service>/actions` is route-scoped, not admin-scoped
- ☐ Link package from `docs/README.md` **and** `docs/design/README.md` (hand-maintained index)
- ☐ Gate: no "advisory only"; no "relaxes elicitation" softening

## Acceptance gate

- ☑ `just lint`
- ☑ `cargo nextest run --workspace --all-features` (2731/2734 initially; three unrelated xtask binary-race failures passed 3/3 on immediate focused rerun)
- ☑ `just docs-generate && just docs-check`
- ☑ `scripts/ci/mcp-conformance.sh`
- ☑ Feature-slice compile checks: `--features gateway` and `--features fs`
- ☑ F9 answered and recorded (Option A)

## Decision log

| Date | Decision | Rationale |
|---|---|---|
| 2026-08-05 | Tool hints are the least-safe union of a service's actions | MCP hints are per-tool; `ActionSpec.destructive` is per-action. |
| 2026-08-05 | Derive `destructiveHint`; keep the other three in a reviewed table | Only `destructive` has a machine-checkable source. |
| 2026-08-05 | Set all four hints explicitly | Deliberate deviation from reference servers, which omit two on read-only tools. Clients that read `destructiveHint` without checking `readOnlyHint` are common. |
| 2026-08-05 | `doctor` is **not** read-only despite zero destructive actions | `system.checks` writes/removes a probe file. |
| 2026-08-05 | Upstream annotations pass through verbatim | Labby relays the claim and gates independently. |
| 2026-08-05 | Passthrough half is tests-only | Verified already implemented. |
| **2026-08-05** | **`server_logs` forced to `destructiveHint: true`** | `requires_admin: true` + key-only redaction; documented override of R2. Do not flip the action-level flag. |
| **2026-08-05** | **Builders live in new `mcp/descriptors.rs`** | 4 of 5 are conditionally advertised, contradicting `permanent_tools`' permanence invariant; avoids a `registry` cycle. |
| **2026-08-05** | **Drop `AnnotationPolicy`; use a free fn + `match`** | `ToolAnnotations` being `#[non_exhaustive]` with non-`const` builders forces an intermediate, but not a struct. |
| **2026-08-05** | **Do not memoize the policy** | Different registries produce different service sets; a name-keyed global would leak across them. |
| **2026-08-05** | **F9 accepted (Option A) — annotate all five, no amendment** | `can_execute` is default-deny on absence of `lab`/`lab:admin`, not on `lab:read`. No client lacks both; stdio resolves to `TrustedLocal`. Inert in this deployment; 5e retained as a regression guard because it rests on config, not an invariant. |
| **2026-08-05** | **Split `.1` into `.1a`/`.1b` by behavior change** | Isolates the hash-moving, gate-widening change into a small revertable diff. |
| **2026-08-05** | **Cut the `_meta` per-action risk map** | Duplicates `lab://<service>/actions` where it matters; peer-accurate form needs ~122 async lock acquisitions per build; discloses the admin-action inventory. |
| **2026-08-05** | **Cut `.4`** | Only real cache stampede in the package; no single-flight on the render/embedding caches. |
| **2026-08-05** | **`doctor` SSRF hardening = companion bead, not prerequisite** | Epic neither creates nor widens the primary-gateway exposure, but does formalize and un-gate it. |

## Corrections applied

| Was | Now |
|---|---|
| `add_server` "delegates to `gateway.test`/`gateway.add`, both destructive-gated" | **False** — both are `destructive: false`. Citation removed. |
| `fs` audit row "0 / 2" | 0 / 1 — MCP registers only `fs.list`. |
| "one-time `tools/list_changed` for every peer on upgrade" | **Cannot happen** — peers seed `last_contract` at registration. |
| "hash changes exactly once vs baseline" | Replaced with two self-describing assertions. |
| C3 "per-action truth is published in resources" | True for 7 service tools only; 5 meta tools have neither resource nor `help`. |
| "compare serialized JSON if `Tool` lacks `PartialEq`" | `Tool` **does** derive `PartialEq` (`tool.rs:13`). |
| Builder line numbers `:122/:128/:134/:140` | `:125/:131/:137/:143`. |
| `with_annotations` test at `catalog.rs:534` | `:535`. |
| "relaxes MRTR elicitation" | Widens **authorization** — F9. |
| Payoff = "clients pre-warn on destructive tools" | `readOnlyHint` is the hint with confirmed client behavior (incl. Claude Code parallelism). |
| `fixture_annotated_upstream_tool` signature | Would not compile; use `&Arc<str>`. |

## Open questions

| # | Question | Owner | Status |
|---|---|---|---|
| 1 | Byte-identical description literals across mirror sites? | — | ☑ **Yes** — no pre-existing hash bug. |
| 2 | Which registry constructor for policy tests? | — | ☑ `build_docs_registry()` (`registry.rs:399`). |
| 3 | Generated artifacts embedding tool JSON? | implementer | ☐ none identified; still run `docs-check`. |
| 4 | Ship `.4`? | — | ☑ No — cut. |
| 5 | F9: accept the widened next-hop reach? | reviewer | ☑ **Yes — Option A.** No client lacks `lab`/`lab:admin`; gate is inert. 5e ships as regression guard. |

## Pre-existing issues found (separate beads)

- `gateway.test` spawns local processes but is `destructive: false` — contradicts root `CLAUDE.md`.
- `doctor` SSRF validator weaker than `labby-primitives::ssrf`.
- `docs/surfaces/MCP.md:66-68` contradicts the code.
- Human/agent parity inverted: `render_catalog` drops per-action `destructive` that `help` keeps.
- No single-flight on `CatalogRenderCache` / `CatalogEmbeddingCache`.
- `evaluate_peers` is a sequential `for` with `.await` inside; sibling notify uses `join_all`.
- `server_logs` redaction is key-based only.

## Coordination

Branches touching the same files — rebase early; `.1a` is designed to rebase cleanly:

| Branch | Overlap |
|---|---|
| `audit/mcp-2026-07-28-capabilities` | `helpers.rs`, `peer_contract.rs` |
| `fix/pool-bulkhead-coverage-20260805` | `helpers.rs` |
| `integration/sync-all-20260802` | `helpers.rs`, `catalog.rs`, `handlers_tools/tests.rs` |
| `feat/google-credential-broker` | `handlers_tools/tests.rs` |
| `preserve/mcp-discovery-deploy-clean-20260802` | `helpers.rs` |

Related open issues: [#208](https://github.com/dinglebear-ai/labby/issues/208),
[#209](https://github.com/dinglebear-ai/labby/issues/209) — complementary
destructive-gate UX. No blocking dependency in either direction.
