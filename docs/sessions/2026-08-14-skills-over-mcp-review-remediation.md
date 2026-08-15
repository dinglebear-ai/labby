---
date: 2026-08-14 21:38:32 EST
repo: git@github.com:dinglebear-ai/labby.git
branch: claude/skills-native-scheme
head: 1c21f9eb2b89a8ba32c27c81a9d258f513c17999
working directory: /home/jmagar/workspace/labby/.claude/worktrees/skills-over-mcp-708560
worktree: /home/jmagar/workspace/labby/.claude/worktrees/skills-over-mcp-708560
pr: "#410 fix(skills): harden native URI aggregation (https://github.com/dinglebear-ai/labby/pull/410)"
beads: lab-hlhwo.25, lab-hlhwo.26, lab-hlhwo.27, lab-hlhwo.28, lab-hlhwo.29, lab-hlhwo.30, lab-puqcw, lab-wryy7
---

# Skills over MCP review and remediation

## User Request

Thoroughly review the newly landed skills-over-MCP implementation, address the selected and newly discovered issues including pre-existing defects, verify the result, and merge it.

## Session Overview

The review found correctness, integrity, performance, and cache-scope defects across URI parsing, aggregation, resource reads, and URI-only `skills/get`. All observed findings were remediated with regression coverage. PR #410 passed its complete CI gate and merged to `main` as squash commit `80a61c570cbaff5058707f9ce548774ede4fec1b`.

## Sequence of Events

1. Reviewed the skills URI, aggregation, list/get, resource-read, cache, and contract paths with Lavra review agents.
2. Confirmed the selected RFC scheme, exclusion-count, and native URI fallback defects and identified collision, metadata, indexing, cache-scope, and fan-out issues.
3. Implemented reversible scheme-aware identities, fail-closed collision handling, indexed reads, metadata parity, cache-scope correction, and concurrent deterministic listing.
4. Added regression tests, reconciled concurrent `main` changes, fixed scoped lint findings, and closed all eight related beads.
5. Ran focused and repository-wide verification, enabled auto-merge, waited for CI, and confirmed PR #410 merged.

## Key Findings

- `crates/labby-runtime/src/skills/uri.rs`: RFC 3986 schemes require case-insensitive comparison; raw prefix checks were not a safe namespace boundary.
- `crates/labby/src/mcp/skills/aggregate.rs`: dropped collisions were missing from completeness accounting, the scan was quadratic, and only top-level URIs were checked rather than all owned resource URIs.
- `crates/labby/src/mcp/skills.rs`: minting erased native schemes, so cache-miss fallback could fail or substitute a same-path `skill://` identity; fallback metadata also differed from listed entries.
- `crates/labby-gateway/src/upstream/pool/skills.rs`: resource reads repeatedly scanned all manifests and identical duplicate bindings could resolve by iteration order.
- Pre-existing issues included public cache scope for subject-dependent empty/incomplete results and sequential upstream listing latency.

## Technical Decisions

- Encode the upstream scheme in the published URI so identity remains reversible without depending on a bounded catalog cache.
- Parse and compare canonical URI components; preserve strict root parsing separately from generic resource parsing.
- Treat duplicate ownership as ambiguous and exclude/fail closed rather than select an iteration-order winner.
- Build canonical indexes with ordered maps/sets for bounded lookup and deterministic output.
- Fetch independent upstream catalogs concurrently, then sort deterministically before aggregation.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/labby-gateway/src/upstream/pool/skills.rs` | — | Indexed, ambiguity-safe resource reads | PR #410 diff |
| modified | `crates/labby-gateway/src/upstream/pool/skills_list.rs` | — | Catalog/index construction | PR #410 diff |
| modified | `crates/labby-gateway/src/upstream/pool/skills_tests.rs` | — | Gateway regressions | 37 focused tests passed |
| modified | `crates/labby-runtime/src/skills.rs` | — | URI API exports | PR #410 diff |
| modified | `crates/labby-runtime/src/skills/manifest.rs` | — | Canonical scheme-bound validation | PR #410 diff |
| modified | `crates/labby-runtime/src/skills/uri.rs` | — | Canonical parsing and reversible identity | 58 unit tests passed |
| modified | `crates/labby-runtime/src/skills/wire.rs` | — | Wire identity behavior | PR #410 diff |
| modified | `crates/labby-runtime/tests/agent_error_schema.rs` | — | Schema regression updates | CI passed |
| modified | `crates/labby-runtime/tests/sep_2640_uri_conformance.rs` | — | SEP URI conformance | 11 tests passed |
| modified | `crates/labby/src/mcp/skills.rs` | — | URI-only get, metadata, concurrency, cache scope | 31 focused tests passed |
| modified | `crates/labby/src/mcp/skills/aggregate.rs` | — | Complete collision accounting | 31 focused tests passed |
| modified | `docs/contracts/skills-extension.md` | — | Updated published contract | `just docs-check` passed |

## Beads Activity

| id | title | actions | final status | why it mattered |
|---|---|---|---|---|
| `lab-hlhwo.25` | Preserve scheme-injective skill identity across list and get | created, implemented, closed | closed | Prevented native identity loss/substitution |
| `lab-hlhwo.26` | Reject skill identity collisions across list get and resource reads | created, implemented, closed | closed | Removed ambiguous ownership |
| `lab-hlhwo.27` | Attach origin and tool-scope metadata to unlisted skill fallback | created, implemented, closed | closed | Restored safety metadata parity |
| `lab-hlhwo.28` | Index proxied skill resources for bounded read lookup | created, implemented, closed | closed | Replaced repeated catalog scans |
| `lab-hlhwo.29` | Replace quadratic minted skill collision scan | created, implemented, closed | closed | Bounded aggregation cost |
| `lab-hlhwo.30` | Clarify generic skill resource URI parsing boundary | created, implemented, closed | closed | Separated root and resource parsing contracts |
| `lab-puqcw` | Downgrade cache scope for subject-dependent incomplete skill listings | created, implemented, closed | closed | Corrected pre-existing privacy semantics |
| `lab-wryy7` | Parallelize cold skills listing across independent upstreams | created, implemented, closed | closed | Corrected pre-existing summed latency |

## Repository Maintenance

- Plans: inspected `docs/plans`; no plan was both session-related and clearly complete, so none was moved. Ambiguous or unrelated plans were left untouched.
- Beads: read and closed all eight directly related items only after implementation and verification were observed; no known skills follow-up remained.
- Worktrees/branches: inspected worktrees and branch tracking. Active, dirty, remote-gone, or ownership-unclear worktrees/branches were not removed; the active squash-merged topic branch was not ancestry-safe to delete automatically.
- Stale docs: updated `docs/contracts/skills-extension.md`; `just docs-check` reported all 17 generated artifacts fresh.
- No unrelated repository state was modified during maintenance.

## Tools and Skills Used

- Shell/file tools: `git`, `gh`, `cargo`, `just`, `bd`, `rg`, process inspection, and patch editing for inspection, implementation, tracking, tests, and merge management.
- Skills/plugins: `lavra:lavra-review` drove the multi-perspective review; test-driven and verification workflows guided regression coverage and completion evidence; `vibin:save-to-md` produced this artifact.
- Review agents: architecture, performance, integrity, agent-native, pattern, history, and simplicity reviewers supplied scoped findings. Findings were consolidated before implementation.
- GitHub CLI: inspected PR state/checks, enabled auto-merge, and verified the merged commit. No browser or external research tool was used.
- Local Labby health was unreachable at `http://localhost:8765/health` during session-note setup; this repository-only workflow did not require the runtime.

## Commands Executed

| command | result |
|---|---|
| `git worktree list --porcelain` and branch/status checks | Established worktree ownership and preserved unrelated state |
| `bd list ...`, `bd create/update/close ...` | Tracked and closed eight findings |
| `cargo nextest run` focused filters | Runtime 58, SEP 11, gateway 37, MCP 31 passed |
| `cargo clippy` focused and CI all-target runs | Focused checks and final CI Clippy passed |
| `just docs-check` | 17 generated artifacts fresh |
| `gh pr checks 410 --watch` | Complete CI gate passed |
| `gh pr view 410 --json ...` | Confirmed merged state and squash commit |

## Errors Encountered

- Early tests exposed stale expectations and a self-collision between a skill root and its own resource; owner-set semantics resolved both.
- Stale Cargo/Kache processes held target locks; only task-owned stale processes were stopped and verification was rerun with `RUSTC_WRAPPER=''` where needed.
- Concurrent `main` integration left an existing merge state and conflicts; conflict stages were inspected and reconciled without overwriting unrelated changes.
- Initial all-target Clippy included scoped test lints and unrelated mainline lints; scoped issues were fixed, later mainline fixes were merged, and final CI Clippy passed.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| URI identity | Native scheme erased during minting | Scheme is encoded and exactly reversible |
| URI-only get | Could fail or query a different same-path scheme | Resolves the precise upstream URI with origin metadata |
| collisions | Partial accounting and top-level-only checks | All owned URIs are indexed; ambiguous owners are excluded/fail closed |
| resource reads | Repeated full manifest scans | Canonical indexed lookup |
| cache scope | Some subject-dependent incomplete results remained public | Such results downgrade scope appropriately |
| cold listing | Upstreams awaited sequentially | Independent fetches run concurrently with deterministic aggregation |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| Runtime skill unit tests | pass | 58 passed | pass |
| SEP URI conformance | pass | 11 passed | pass |
| Gateway skills tests | pass | 37 passed | pass |
| MCP skills tests | pass | 31 passed | pass |
| `just docs-check` | fresh | 17 artifacts fresh | pass |
| PR #410 CI run `31853831383` | all required jobs green | full test, Clippy, format, deny, docs, slices, conformance, regressions, and `ci-gate` passed | pass |
| PR merge inspection | merged into `main` | squash commit `80a61c570cbaff5058707f9ce548774ede4fec1b` | pass |

## Risks and Rollback

- The published proxied URI shape changed to preserve native schemes; clients retaining identities from the superseded shape should relist.
- If rollback is required, revert squash commit `80a61c570cbaff5058707f9ce548774ede4fec1b` on `main`; doing so also restores the previously documented correctness and ambiguity defects.

## Decisions Not Taken

- Did not retain a cache-only downstream-to-upstream mapping because URI-only loading must survive cache churn and truncation.
- Did not guess an erased upstream scheme or choose a collision winner by iteration order.
- Did not delete active or ownership-unclear worktrees/branches, or move ambiguous plans.

## References

- [PR #410](https://github.com/dinglebear-ai/labby/pull/410)
- `docs/contracts/skills-extension.md`
- RFC 3986 scheme comparison requirements and SEP URI conformance tests embodied in the repository test suite.

## Next Steps

No unfinished skills-over-MCP remediation remains from this session. Normal follow-through is to consume the merged `main`, relist skills where clients retained pre-change proxied URIs, and monitor downstream integration/release checks.
