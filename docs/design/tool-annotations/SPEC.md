---
title: "Tool Annotations Specification"
created: "2026-08-05"
updated: "2026-08-13"
---

# Specification — Tool Annotations

Status: implemented · Issue: [#212](https://github.com/dinglebear-ai/labby/issues/212) · Epic: `lab-g1av5`

> Read [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md) alongside this document. It records
> what changed and why, including one **gating unknown (F9)** that must be settled
> before annotations are switched on.

## 1. Problem

MCP clients cannot currently tell a read-only Labby tool from a mutating one.
Two concrete gaps:

1. **Labby's own tools carry no hints.** Every `Tool` Labby constructs leaves
   `annotations: None`. The only `with_annotations` call in the workspace is a
   unit test (`crates/labby/src/mcp/catalog.rs:535`). Clients cannot separate
   read from write, and cannot apply read-only fast paths.

   **What the payoff actually is.** `readOnlyHint` is the hint with confirmed
   client behavior: VS Code skips its confirmation prompt for read-only tools,
   ChatGPT renders a READ/WRITE badge, and Claude Code gates **parallel tool
   execution** on it. No official client documentation confirms `destructiveHint`
   producing a distinct, stronger warning today. So the concrete win is
   concurrency and read/write rendering for `fs`, `server_logs`, and
   `lab_admin` — not the "pre-warn on destructive tools" framing in the issue.
   The other three hints are set for spec compliance and forward compatibility.
2. **Upstream passthrough is unverified.** The gateway aggregates upstream tools
   into its own `tools/list`. Nothing asserts that an upstream's annotations
   survive that aggregation, so a future refactor could silently drop them.

## 2. Goals

- G1. Every Labby-owned MCP tool advertises all four MCP hints explicitly.
- G2. `destructiveHint` for service tools is derived from the live action
  catalog, so it cannot drift from `ActionSpec.destructive`.
- G3. Upstream annotations are proven to reach downstream clients byte-identical,
  on every listing path, including through two gateway hops.
- G4. The wire listing and peer-contract listing cannot silently diverge.

## 3. Non-goals

- N1. Adding `read_only` / `idempotent` / `open_world` fields to `ActionSpec`.
  Per-action hints are a plausible future; this epic does not open it.
- N2. Changing the fail-closed derivation in `cached_upstream_tool`.
- N3. Changing the MRTR elicitation gate for local (non-proxied) actions.
- N4. Normalizing, rewriting, or overriding upstream-supplied annotations.

## 4. Core design constraint: granularity mismatch

| Layer | Granularity | Source |
|---|---|---|
| MCP `ToolAnnotations` | per **tool** | this spec |
| Labby `ActionSpec.destructive` | per **action** | `crates/labby-primitives/src/action.rs:9` |
| Labby MCP surface | one tool = one **service** | `crates/labby/src/mcp/CLAUDE.md` |

A service such as `setup` has 16 destructive actions out of 32. Its single tool
can only honestly advertise the **least-safe union** of its actions. This is
accepted, not worked around. rmcp documents the same caveat
(`rmcp-3.1.0/src/model/tool.rs:44-49`): annotations are hints, never guarantees.

Consequence for clients: a client that pre-warns on `destructiveHint` will warn
on *every* `setup` call, including read-only ones. That is the honest signal at
tool granularity; finer granularity requires N1, which is out of scope.

## 5. Mutation audit

`readOnlyHint: true` is a strong claim — and in gateway chains it is load-bearing
(§7). Each claim below is backed by reading the dispatch implementation, not by
the destructive-flag count alone. **"Zero destructive actions" does not imply
"read-only."**

| Service | Destructive | Admin-only | Read-only? | Evidence |
|---|---|---|---|---|
| `fs` | 0 / 1 | 0 | **yes** | MCP registers only `fs.list` (`mcp/services/fs.rs`). `fs.preview` is excluded (`registry.rs:494-503`) and rejected at the MCP surface (`dispatch/fs/dispatch.rs:83-87`). |
| `server_logs` | 0 / 3 | **1** | yes, **but see § 6** | Only `server_logs.query` — but it is `requires_admin: true` (`dispatch/server_logs/catalog.rs:29`) and redaction is key-based only (`labby-runtime/src/redact.rs:29-64`), never scanning free-text `message` values. |
| `lab_admin` | 0 / 3 | 0 | yes, **by vacuity** | `onboarding.audit` is declared but **unimplemented** — `dispatch/lab_admin/dispatch.rs:59-72` matches only `help`/`schema`. Re-audit when implemented. |
| `gateway_status` (meta) | n/a | all | **yes** | Renders live upstream status; no mutation path. |
| `doctor` | 0 / 8 | 0 | **no** | `system.checks` writes and removes a probe file — `dispatch/doctor/system.rs:41-42` (`.doctor_write_test`). Mutating but non-destructive. |
| `mcp_app` (meta) | n/a | mutations | **no** | Toggles the Code Mode app surface (`status\|enable\|disable`). Mutating, reversible, idempotent. |
| `snippets` | 2 / 10 | — | **no** | `snippets.promote`, `snippets.remove`. |
| `setup` | 16 / 32 | 26 / 32 | **no** | Config/env writes, repair actions. |
| `gateway` | **12 / 64** | **61 / 64** | **no** | `gateway.code_mode.set`, `enrich.preview`, `enrich.apply`, `remove`, `test`. |
| `codemode`, `codemode_ui` | n/a | — | **no** | Execute snippets that invoke arbitrary upstream tools. |
| `codemode_read` | n/a | — | **yes** | Restricted to explicitly read-only upstream tools; artifact writes are disabled. |
| `add_server` (meta) | n/a | — | **no** | Persists gateway config and can spawn a local subprocess. |

Three lessons this audit produced:

- **`doctor`** — a naive "0 destructive ⇒ read-only" rule would have advertised a
  false read-only claim for a tool that writes to disk. "Non-destructive" ≠
  "read-only".
- **`server_logs`** — counting `destructive` flags is not sufficient. `requires_admin`
  is a second axis and confidentiality is a third; see § 6.
- **A corrected claim.** An earlier draft justified `add_server` with
  "`gateway.test` / `gateway.add`, both destructive-gated". That is **false** —
  both are `destructive: false` (`gateway/catalog.rs:527-529`, `:706-708`). The
  `destructiveHint: true` value stands on its own merits. This surfaced a
  **separate pre-existing bug**: `gateway.test` spawns local processes yet is
  flagged non-destructive, contradicting the root `CLAUDE.md` policy that names
  it explicitly. Filed separately; not fixed here.

**Over-warn rate.** Because a tool-level hint is the union of its actions,
`destructiveHint: true` is a false positive for most calls: `gateway` 12/64,
`setup` 16/32, `snippets` 2/10 — aggregate ≈ **72%**. Inherent at tool
granularity; see § 4.

## 6. Decision table (normative)

Implementations MUST NOT re-derive these values. `destructiveHint` for the seven
service tools is computed at runtime and MUST equal the table.

| Tool | `readOnlyHint` | `destructiveHint` | `idempotentHint` | `openWorldHint` |
|---|---|---|---|---|
| `fs` | `true` | `false` | `true` | `false` |
| `server_logs` | **`false`** | **`true`** | `false` | `false` |
| `lab_admin` | `true` | `false` | `true` | `false` |
| `gateway_status` | `true` | `false` | `true` | `false` |
| `doctor` | `false` | `false` | `true` | `true` |
| `mcp_app` | `false` | `false` | `true` | `false` |
| `setup` | `false` | `true` | `false` | `true` |
| `gateway` | `false` | `true` | `false` | `true` |
| `snippets` | `false` | `true` | `false` | `true` |
| `codemode` | `false` | `true` | `false` | `true` |
| `codemode_ui` | `false` | `true` | `false` | `true` |
| `codemode_read` | `true` | `false` | `true` | `true` |
| `add_server` | `false` | `true` | `false` | `true` |

Rationale for the less obvious cells:

- **`doctor.openWorldHint: true`** — `doctor.proxy.check` accepts arbitrary
  `app_url` / `mcp_url` / `backend_url` values and probes them.
- **`fs.openWorldHint: false`** — bounded by the local workspace.
- **`codemode.idempotentHint: false`** — a snippet may call any upstream tool.
- **`mcp_app.idempotentHint: true`** — enabling an already-enabled surface is a
  no-op.

**`server_logs` is a deliberate, documented override of R2.** Its only action is
`destructive: false`, so pure derivation would yield `destructiveHint: false`.
It is forced to `true` — and moved out of the read-only bucket — because
`server_logs.query` is `requires_admin: true` while redaction is key-based only,
and because `destructiveHint: true` is what re-tightens the next-hop gate this
epic otherwise relaxes (see § 7). Do **not** instead flip the action-level
`ActionSpec.destructive`: that would impose local MRTR and CLI `-y` friction on
every `server_logs.query` call everywhere, conflating confidentiality risk with
mutation risk. The override must carry a comment and its own test.

Verification that derivation agrees with the table: `doctor` 0, `fs` 0,
`lab_admin` 0 → `false`; `snippets` 2, `setup` 16, `gateway` 12 → `true`.
Six of the seven registry-backed service rows follow derivation; `server_logs`
is the one documented exception above. Synthetic tools use reviewed constants.

**A deliberate deviation from ecosystem convention.** Setting all four hints
explicitly is *not* what reference implementations do — the official filesystem
server and FastMCP omit `destructiveHint`/`idempotentHint` on read-only tools,
leaning on the spec's "meaningful only when `readOnlyHint == false`" clause, and
rmcp's `skip_serializing_if` defaults toward omission. We set all four anyway
because clients that read `destructiveHint` without first checking `readOnlyHint`
are common, and an explicit `false` is the safer failure mode.

## 7. Safety semantics (must be stated correctly)

Annotations are **advisory to clients, and consumed by Labby itself at the next
hop.** Writing "advisory only" in any shipped doc is a spec violation.

In a labby → labby chain, the downstream gateway calls `cached_upstream_tool`
(`crates/labby-gateway/src/upstream/pool/helpers.rs:423`), which fails closed:

```text
destructive = true, unless annotations exist AND
              (destructive_hint == false, OR
               destructive_hint absent AND read_only_hint == true)
```

Today Labby's builtins are unannotated ⇒ downstream treats them all as
destructive. After this change, the five remaining non-destructive tools (`fs`,
`lab_admin`, `gateway_status`, `doctor`, `mcp_app` — `server_logs` is now
excluded per § 6) become non-destructive at the next hop.

### F9 — the reach is authorization, not just confirmation (**RESOLVED — accepted**)

> **Decision (2026-08-05): accepted; annotate all five. No amendment.**
>
> The Code Mode gate is `destructive_permitted(Mcp, c) == c.can_execute()`, and
> `can_execute` is default-deny on the *absence* of both `lab` and `lab:admin`
> (`call_tool_codemode.rs:807-815`) — it is **not** keyed on `lab:read`. Operator
> confirmed no OAuth client on the gateway holds a scope set lacking those two,
> and the other three paths are already safe: no-auth/stdio resolves to
> `TrustedLocal` (`call_tool_codemode.rs:542`), and the default static-bearer
> scopes are `["lab:read", "lab:admin"]` (`labby-auth/src/config.rs:242`).
> So `can_execute()` is true for every real caller and the widened reach is
> **inert in this deployment**.
>
> **Conditional, not unconditional.** This rests on a deployment fact, not an
> invariant. The codebase already models narrower callers — `palette.rs:186-200`
> routes any non-`lab:admin` caller to `scoped_read_only { can_execute: false }`
> and derives `gateway:<upstream>` scopes — and both `static_token_scopes`
> (`config.rs:454`) and the OAuth `default_scope` are operator-configurable. The
> day a client is issued a scope set without `lab`/`lab:admin`, the reach below
> becomes live. **Keep the Phase 5e gating tests as a standing regression guard**
> so that change fails CI instead of silently widening access.

The mechanism, retained for the record:

An earlier draft described this as "relaxing MRTR elicitation". That
understates it. `UpstreamTool.destructive` also gates:

- widget callbacks — `crates/labby/src/mcp/call_tool.rs:1175`
- the palette — `crates/labby-gateway/src/gateway/palette.rs:235-247`
  (`forbidden` unless `destructive_permitted` **and** `confirm_destructive`)
- Code Mode — `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs:90-107`,
  a hard `forbidden`, where `destructive_permitted(Mcp, c) == c.can_execute()`
  (`crates/labby-codemode/src/types.rs:800-804`)

So today a `lab:read` caller at hop 2 is **forbidden** from every proxied Labby
builtin. After this change those five tools become **callable by a non-execute
caller** — including `mcp_app` enable/disable and `doctor.proxy.check`.

Settled — see the decision box above. The Phase 5e in-process tests still ship,
now as a **regression guard** rather than a gate: they assert, per flipped tool,
what a caller with `can_execute() == false` may invoke, so introducing a
narrow-scoped client later fails CI instead of silently widening access.

Note the plan's original multihop assertion for this is **not implementable**:
`mcp_multihop_conformance` runs out-of-process and `UpstreamTool.destructive`
never crosses the wire. Assert annotation survival there; prove gating in-process.

Unchanged by this epic: the fail-closed rule itself, and the local elicitation
gate driven by `ActionSpec.destructive` (verified — `call_tool.rs:959-1014`
consults only `ActionSpec.destructive`; only the proxied branch at `:1050-1064`
reads the annotation-derived value).

## 8. Requirements

| ID | Requirement |
|---|---|
| R1 | Every Labby-owned tool sets all four hints explicitly; `title` stays unset. |
| R2 | `destructiveHint` for service tools = `svc.actions.iter().any(\|a\| a.destructive)`. |
| R3 | `readOnly`/`idempotent`/`openWorld` come from one reviewed constant table. |
| R4 | A service absent from the table falls back to the least-safe shape (`false,true,false,true`). |
| R5 | Annotation derivation is a pure function of static catalogs — never per-peer, time-, or environment-dependent. |
| R6 | Both construction sites emit identical descriptors, enforced by a shared helper **and** an automated equality test. |
| R7 | A `readOnlyHint: true` service must have zero destructive actions; enforced by test so adding one later fails CI. |
| R8 | Upstream annotations pass through verbatim on every listing path, including subject-scoped OAuth and two-hop proxying. |
| R9 | No change to `cached_upstream_tool`, MRTR elicitation, or `ActionSpec`. |

## 9. Acceptance criteria

- [ ] `cargo nextest run --workspace --all-features` green.
- [ ] `just lint` green (clippy `-D warnings`, fmt, skill-drift, toolchain sync).
- [ ] `just docs-check` green with any generated artifacts regenerated, not hand-edited.
- [ ] `scripts/ci/mcp-conformance.sh` and `mcp-regressions` green.
- [ ] The pre-existing tests `cached_upstream_tool_fails_closed_without_destructive_annotations`
      and `cached_upstream_tool_honors_explicit_non_destructive_hints` pass **unmodified**.
- [ ] Contract hash is stable across two identical in-process builds, and differs
      when annotations are stripped from the same descriptor set. (Replaces the
      earlier "changes exactly once versus a hard-coded baseline", which is only
      assertable against a magic constant and breaks on any unrelated
      description or schema edit.)
- [ ] F9 (§ 7) resolved with in-process gating tests before `.1b` lands.

## 10. Known consequences

1. **There is no `tools/list_changed` storm.** An earlier draft claimed a
   one-time notification to every connected peer on upgrade. That **cannot
   happen**: `server.rs:531-546` seeds `RegisteredPeer.last_contract` at
   registration and `catalog_notifications.rs:342-350` notifies only on a diff
   against that seed. Annotations derive from `&'static` data, so they change
   only across a binary change → process restart → all sessions destroyed →
   every reconnecting peer seeds from post-change descriptors. Do **not** put
   this in release notes: pre-announcing benign churn would train operators to
   dismiss `catalog_churn.rs`'s divergence WARN, which is the real signal for a
   genuine mid-rollout hash split.
2. **Widened next-hop reach** for the five remaining non-destructive builtins —
   authorization, not just confirmation. See § 7 F9.
3. **Deliberately left open:** Code Mode's in-sandbox catalog carries no hints
   and `tool_shape_digest` (`gateway/code_mode/search.rs:25-34`) ignores
   annotations. The stretch bead that would fix it is **cut** — it carries the
   package's only real cache stampede (`CatalogRenderCache` is a single slot with
   no single-flight, `manager.rs:134`, and its sibling embedding cache issues
   batched TEI network calls on miss). Consequence worth recording: `codemode`'s
   `destructiveHint: true` conveys nothing to a spec-compliant client, since
   rmcp's documented default for that field is already `true`.
