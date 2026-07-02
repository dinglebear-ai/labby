---
date: 2026-07-02 04:12:19 EST
repo: git@github.com:jmagar/labby.git
branch: feat/gate-base-services
head: 2748821a
plan: docs/superpowers/plans/2026-07-02-base-service-feature-gating.md
working directory: /home/jmagar/workspace/lab/.worktrees/feat-gate-base-services
worktree: /home/jmagar/workspace/lab/.worktrees/feat-gate-base-services
pr: "#171 feat: gate base services (stash, acp, nodes) for gateway-only builds — https://github.com/jmagar/labby/pull/171"
beads: lab-45uob, lab-c3x6u, lab-sdmbi, lab-aq646 (+4 follow-up beads)
---

# Work-it session: base-service feature gating (PR #171)

## User Request

Starting from "inventory all the code that would be unused without the gateway," the goal inverted to: cut everything that is NOT the gateway. A plan was written (`/writing-plans`) to gate the three ungated base services (`stash`, `acp`, `nodes`) behind cargo features, then `/vibin:work-it` was invoked to execute it to completion in a tracked PR. Mid-session the user also directed a second work-it track for PR #172 (codemode semantic search) in its dedicated checkout.

## Session Overview

PR #171 delivers a gateway-only build shape: `cargo build -p labby --no-default-features --features gateway` now excludes ~32k LOC of fleet/ACP/stash code and the `agent-client-protocol` dependency, while the default all-features build is behaviorally unchanged. Implementation (5 commits) went through three review waves (10 reviewer agents total) plus CodeRabbit (zero findings); all actionable findings were fixed in two batch commits. CI green at session-log time except a handful of pending long jobs. A parallel track implemented PR #172 (semantic search for `codemode.search()`); its review wave was still concluding at log time.

## Sequence of Events

1. Inventoried gateway-only code (3 Explore agents), then inverted to a cut-list (3 more agents + verified `--features gateway` compiles).
2. Wrote the implementation plan; user review removed docs-gen from the cut list (registry-driven, documents the gateway itself).
3. work-it: discovered the session's nominal worktree was phantom and the main checkout was on another agent's branch; created `.worktrees/feat-gate-base-services` off `origin/main` via `worktree-setup` (warm-synced), re-verified every plan anchor against main (one fix: `marketplace` already requires `gateway`).
4. Draft PR #171 created; implementation agent executed the plan (Tasks 0-4, bead `lab-45uob`, commits `d14db2a0`, `7bceae88`, `84f3b69b`, `2a3dbc51`).
5. Two CI failures diagnosed and relayed mid-run: marketplace slice needs `nodes` (uses `dispatch::node::send` for remote installs); generated `feature-matrix` stale — both fixed in Task 4's commit.
6. Review wave 1 (lavra, 5 agents): 1 P1 + 3 P2 + P3s → fix batch `7c65746d` (bead `lab-c3x6u`, 9 items). Goal-verifier: PASS on all plan criteria; security: auth topology byte-identical.
7. `cli-help.md` staleness (from the help-text fix) regenerated and pushed as `7606659f`.
8. Review wave 2 (PR toolkit, 5 agents): headline findings — omit-side tests never execute (slice CI is check-only), settings surface still accepts config for compiled-out subsystems. Fix batch `2748821a` (bead `lab-sdmbi`, 11 items incl. gateway-slice nextest CI step which exposed and fixed 7 pre-existing test failures in that shape).
9. Review wave 3 (delta): no actionable findings — diminishing returns declared. PR marked ready; CodeRabbit review triggered → finished with zero inline findings; Copilot/Codex quota-limited.
10. Parallel track: PR #172 implemented by a second agent (7 tasks, 8 commits, bead `lab-aq646`, live TEI smoke matrix); one CI lint fix (`27ad64d2`) applied by coordinator; review wave dispatched (security P1: unmetered `__lab_internal` TEI amplification — fix batch pending at log time).

## Key Findings

- The `gateway` cargo feature slice already compiled clean before this work; the fat was the ungated base services (acp ~12.7k, nodes ~12.1k, stash ~6.9k LOC).
- `marketplace` transitively requires `gateway`+`acp`+`nodes`+`stash` — each coupling verified to a specific file (`stash_bridge.rs`, `acp_dispatch.rs`, `dispatch/node/send.rs`).
- CI slice jobs are `cargo check --all-targets` only, so `#[cfg(not(feature))]` tests were dead weight until the new gateway-slice nextest step (`.github/workflows/ci.yml:225-229`).
- The always-on settings surface (`dispatch/setup/settings.rs`) accepted and persisted config for compiled-out subsystems — including a `node.role` write that would brick the next restart.
- Public fleet routes (`/v1/nodes/hello`, ws) are auth-exempt by construction (own sub-router, no route_layer) — now pinned by test (`tests/nodes_api.rs:337`) and comment (`api/router.rs:1553`).

## Technical Decisions

- Feature names `stash`/`acp`/`nodes`, all members of `all`; `acp` owns `dep:agent-client-protocol`. Docs generation deliberately NOT gated (user call: registry-driven, needed by the gateway build).
- `node/identity.rs` stays always-on (serve role resolution + router hostname); everything else in `src/node/` gated.
- Config-without-feature fails loud (`resolve_node_role_without_nodes`) or warns (ignored `[node]` keys, ACP env vars) — mirroring `reject_protected_routes_without_gateway`.
- New `FeatureClass::BaseCapability` in the docs classifier so the generated feature-matrix stops labeling `acp` as "helper/internal".

## Files Changed

| status | path | purpose |
|---|---|---|
| created | docs/superpowers/plans/2026-07-02-base-service-feature-gating.md | implementation plan |
| modified | crates/labby/Cargo.toml | features `stash`/`acp`/`nodes`; optional ACP dep; marketplace deps; contract comment |
| modified | crates/labby/src/{lib,main,dispatch,cli,node,registry,api,api/router,api/state,api/services,cli/serve,cli/logs,dispatch/logs,mcp/services}.rs | cfg gates, rejection/warn helpers, tests |
| modified | crates/labby/src/dispatch/setup/settings.rs, dispatch.rs | settings-field gating, env-schema filter, test fixes |
| modified | crates/labby/src/docs/{projection,types}.rs | BaseCapability classifier |
| created | crates/labby/tests/node_identity.rs | un-trapped always-on identity tests |
| modified | crates/labby/tests/* (13 files) | `#![cfg(feature = ...)]` headers, auth-exemption pin test, import split |
| modified | .github/workflows/ci.yml | +acp/nodes/stash/"nodes,deploy" slices; gateway-slice nextest step |
| modified | CLAUDE.md, Justfile, docs/dev/SERVICES.md, docs/coverage/stash.md, docs/acp/{design,README}.md, docs/services/STASH.md, crates/labby/src/dispatch/CLAUDE.md, crates/labby-apis/src/{acp,stash}.rs, crates/labby/src/cli/marketplace.rs | doc/comment accuracy |
| modified | docs/generated/* (feature-matrix, cli-help, service-catalog, SERVICES) | regenerated |

## Beads Activity

| id | title | action | status |
|---|---|---|---|
| lab-45uob | Feature-gate base services | created, claimed, closed by impl agent | closed |
| lab-c3x6u | review wave-1 fix batch | created by coordinator, claimed+closed by fix agent | closed |
| lab-sdmbi | review wave-2 fix batch | created+claimed+closed by fix agent | closed |
| lab-aq646 | codemode semantic search (PR #172 track) | created+claimed+closed by impl agent | closed |
| (new) | Web UI feature-unawareness (/v1 pages 404 opaquely) | created, P2 follow-up | open |
| (new) | Structured 404 envelope for compiled-out routes | created, P3 follow-up | open |
| (new) | Feature-table cleanup (node-runtime, services-all, ALWAYS_VISIBLE_SERVICES) | created, P3 follow-up | open |
| (new) | build_env_schema swallows env-reference parse failure | created, P3 follow-up | open |

Knowledge captured on lab-45uob: PATTERN (feature gates need omission tests), LEARNED (marketplace→nodes coupling; web UI lacks capability discovery).

## Repository Maintenance

- Plans: the gating plan is complete but was NOT moved (quick-push prohibits plan moves mid-flow) — follow-up: move to a `complete/` location per repo convention.
- Beads: all session beads closed except the four follow-ups (intentionally open, tagged in descriptions as non-blocking).
- Worktrees/branches: none removed — all observed worktrees are active (other sessions own `codemode-wasmtime-dual-sandbox`, `wizardly-hofstadter`, codex worktrees). This worktree stays until PR #171 merges.
- Stale docs: handled in-band by review waves (see Files Changed).

## Tools and Skills Used

- Skills: writing-plans, work-it, worktree-setup, lavra-review, review-pr (toolkit sweep), quick-push, save-to-md.
- Agents: 3+3 Explore (inventory), 2 implementation (executing-plans), 2 fix agents, 10 review agents across 3 waves (+4 on PR #172), all via background dispatch with SendMessage steering.
- Shell/git/gh throughout; `bd` for beads. Issues: shell cwd resets between calls (worked around with `cd` prefixes); phantom session worktree discovered and abandoned; shared `target/` caused one contaminated test run and a foreign-binary smoke-test trap on the parallel track (documented workarounds); `bd create --type=improvement` invalid (used task/feature); one docs-regen command hit the 2-minute Bash timeout (split and re-run).

## Commands Executed

| command | result |
|---|---|
| `cargo check -p labby --no-default-features --features gateway --all-targets` | pass (multiple runs) |
| `just lint` / `just test` | pass; final 2333 tests all-features |
| `RUSTFLAGS="" cargo nextest run -p labby --no-default-features --features gateway` | 798 pass (was 7 failures before wave-2 fixes) |
| `cargo run -p labby --all-features -- docs check` | 15 artifacts fresh |
| `gh pr create/edit/ready/checks/comment` | PR #171 lifecycle |

## Errors Encountered

- CI "Feature slice (marketplace)" failure — missing `nodes` feature dep; fixed via Cargo.toml.
- CI "Generated docs" failures (2×) — feature-matrix then cli-help staleness; regenerated.
- CI "Test" failure on PR #172 — `unused_qualifications` under `-D warnings`; one-line fix `27ad64d2`.
- Shared-target contamination: one `just test` run saw phantom `semantic_rank` errors from the sibling branch's artifacts; `cargo clean -p labby-codemode -p labby-gateway` resolved.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| gateway-slice cargo tree grep agent-client-protocol | empty | empty | pass |
| `labby stash/nodes --help` (gateway slice) | unrecognized subcommand | unrecognized subcommand | pass |
| `labby docs --help` (gateway slice) | still present | present | pass |
| `just test` all-features | all pass | 2333 pass / 13 skip | pass |
| gateway-slice nextest | all pass | 798 pass / 4 skip | pass |
| PR #171 CI at log time | green | 33 pass / 5 pending / 0 fail | pending |

## Risks and Rollback

- Risk: lean-build shapes are new; some behavior (settings gating, warns) only exercised by the new gateway-slice CI step. Rollback: revert the branch; features are additive and `all` is unchanged, so reverting restores exact prior behavior.
- Open architectural debt (beaded, non-blocking): web UI is feature-unaware; compiled-out routes return bare 404.

## Next Steps

1. Wait for the 5 pending CI jobs on `2748821a`; merge PR #171 when green (merge-status gate to run first).
2. PR #172: land the security fix batch (P1 internal-call cap, P2 query clamp + bounded response read, P3 shared formula helper + shared reqwest client), finish its review waves, session-log it, merge-status, merge.
3. Follow-ups (beaded): web-UI capability discovery; structured `feature_not_compiled` envelope; feature-table cleanup; env-schema warn.
4. Not started: the web-UI trim (~27k TS) and the two-binary workspace split from the original cut-list analysis.
