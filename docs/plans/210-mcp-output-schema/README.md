# Issue #210 — MCP `outputSchema` + `structuredContent`

Design and implementation package for
[dinglebear-ai/labby#210](https://github.com/dinglebear-ai/labby/issues/210).

| | |
|---|---|
| Epic bead | `lab-41e7m` (children `.1`–`.4`, plus security beads to file) |
| Branch | `feat/mcp-output-schema-210` |
| Base | `origin/main` @ `132448802` |
| Phase | Planning **revised after a 10-agent research pass** — no code written yet |
| MCP revisions | 2025-06-18 → 2026-07-28 |
| SDK | `rmcp` `=3.1.0` (no bump; all 9 API claims verified) |

---

## Read in this order

| # | Document | Answers |
|---|---|---|
| 1 | [SPEC.md](SPEC.md) | What we're building, what already exists, what's out of scope, and why. **Acceptance criteria live here (§6) — single source.** |
| 2 | [CONTRACT.md](CONTRACT.md) | Normative wire contract (MUST/SHOULD) per tool class |
| 3 | [RESEARCH.md](RESEARCH.md) | The 10-agent evidence behind every revision |
| 4 | [SCHEMAS.md](SCHEMAS.md) → [`schemas/`](schemas/) | Two JSON Schema artifacts and their provenance |
| 5 | [MODELS.md](MODELS.md) | Rust types on the path, with definition sites |
| 6 | [TYPES.md](TYPES.md) | Wire ↔ Rust ↔ TypeScript views |
| 7 | [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | Step-by-step with real code |
| 8 | [PROGRESS.md](PROGRESS.md) | Status, audit table, decisions D1–D13, findings F1–F23 |

---

## The one-paragraph version

Verification found that **most of #210 is already implemented**: every dispatch result already
sets `structuredContent`, the Code Mode unwrap already prefers it (byte-identical since
2026-05-31), and the catalog already carries upstream `output_schema` and renders it as typed
`Promise<T>`. The genuine gaps are narrower: builtin and synthetic tools advertise no
`outputSchema`; descriptor and gating logic is duplicated across **four** sites; the unwrap is
correct but unpinned; and the Code Mode trace schema is inconsistent with its own error path.
A research pass then found something the issue's framing obscures — under Code Mode, the tools
being given schemas are **hidden from `tools/list` entirely**, so this is a Raw-mode improvement
and must be labelled as one.

## What changes

| Area | Change | Bead |
|---|---|---|
| Builtin + synthetic tools | Success-envelope `outputSchema`, after a two-axis audit | `.1` |
| Descriptor construction | Extend the **existing** `PermanentToolRegistry`; extend the **existing** drift test with a Raw-mode fixture | `.1` |
| Authorization gates | Collapse duplicated `add_server`/`gateway_status`/`action_allowed` chains onto `PeerContract` | `.1` |
| Code Mode trace | Add `logs_count: 0` to the error trace | `.1` |
| Code Mode unwrap | Document precedence in code; pin with an edge-case matrix | `.2` |
| Truncation / proxy | Cover **both** truncation markers; success-path fidelity | `.2` |
| Catalog | Verify upstream shapes reach `.d.ts`; document snippets; audit OpenAPI | `.3` |
| Docs | `MCP.md`, `CODE_MODE.md`, promote the contract to `docs/contracts/` | `.4` |
| **Security** | Sanitize upstream metadata on `tools/list`; bound `$ref` expansion | **new** |

**No** new error kinds. **No** `rmcp` bump. **No** envelope change. **No** unwrap behavior change.

## Load-bearing decisions

- **This is a Raw-mode improvement.** Builtins are suppressed from `tools/list` whenever Code
  Mode is enabled, so under Code Mode this adds exactly one schema (`server_logs`). Do not ship
  it described as "output shapes now reach agents." (SPEC §2.1)
- **Extend existing seams.** Both the descriptor builder and the drift test already exist;
  the minimal change and the consolidating change turn out to be the same change. (SPEC §5.6)
- **Success-only schema.** No spec text exempts `isError` results — it's converged convention,
  and Labby's own error envelope is separately schema-locked and *rewraps* success-shaped
  content, so one schema cannot describe both. (CONTRACT §C3.2)
- **Audit both shape and presence.** Declaring `outputSchema` without always returning
  `structuredContent` is a hard client-side error in the Python SDK — it already broke Claude
  Code's own Bash tool. (SPEC FR-3)
- **`data` stays open, and that's a limitation, not a mitigation.** No mechanism currently exists
  for an agent to learn an action's result shape. (SPEC NG-1)

## Verification

```bash
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
just docs-generate && just docs-check
```

All-features is the truth; feature-slice warnings are diagnostic only. `docs-check` is expected
to be a no-op — generated artifacts render from the registry action catalog, not from
`rmcp::model::Tool`.

## Working on this

```bash
cd /home/jmagar/workspace/labby/.worktrees/feat-mcp-output-schema-210
bd ready
bd update lab-41e7m.1 --claim
```

`.1`, `.2`, `.3` touch disjoint files (overlapping crates — coordinate any `Cargo.toml` edit);
`.4` is blocked on all three. **`code_mode_trace_output_schema` must stay in `handlers_tools.rs`
for this epic** — `.2` asserts against it while `.1` refactors that file.

Keep commits path-limited: `target/`, `dist/`, and `apps/palette-tauri/*` show as untracked
because the sync engine symlinked them past `.gitignore`'s directory patterns.

## Provenance

Grounded in the tree at `132448802`, the vendored `rmcp` 3.1.0 source, the MCP specification
(2025-06-18 through 2026-07-28), git history, institutional memory, and Cloudflare's
[`unwrapMcpResult`](https://github.com/cloudflare/agents/blob/1bca2a62435dee1a75914c8840d028b832913d0f/packages/codemode/src/mcp.ts).
File:line citations were verified against this worktree. **Two rounds of example-test defects
were found and fixed** (PROGRESS F21, F24): the first draft invented the harness helpers
(`test_request_context`, `call_tool_for_test`); the second fixed the harness but asserted a
service name — `"gateway"` — that `completion_test_registry()` does not register. Both are now
corrected against the real fixture (`hidden-upstream`, `gateway-alpha`, `danger`). Treat example
code in this package as reviewed-but-uncompiled: **run it before trusting it.**
