---
date: 2026-07-02 15:05:09 EST
repo: git@github.com:jmagar/labby.git
branch: session-log/2026-07-02-incus-migration-deploy (log); work spanned main + feat/gate-base-services + integrate/incus-clean-break + fix/cargo-lock-0300 + fix/provision-uv-config
head: 52a2e891
working directory: /home/jmagar/workspace/lab
pr: "#177 fix(setup): uv-python provision fails on /root/uv.toml — https://github.com/jmagar/labby/pull/177 (merged); this log lands via its own docs PR"
beads: lab-45uob, lab-c3x6u, lab-sdmbi (gating); lab-aq646, lab-7k11y (codemode); lab-1x61l (uv provision, closed); lab-x4mw2 + siblings (follow-ups, open)
---

# Gateway-only inventory → base-service gating + semantic search → Incus migration merge + live deploy

## User Request

Started as "inventory all the code that would be unused without the gateway," inverted to "cut everything that isn't the gateway," which became a plan to feature-gate the ungated base services. Then: run it via `/vibin:work-it`; also work a second plan (codemode semantic search, PR #172); then set the TEI URL, merge everything, and rebuild + sync to the Incus gateway — ending with "make the full setup flow work without you manually doing shit."

## Session Overview

Feature-gated `stash`/`acp`/`nodes` behind cargo features for a lean gateway-only build (#171) and blended TEI semantic search into `codemode.search()` (#172), each via a full work-it flow (implementation agent + 3 review waves). Discovered the live Incus gateway container was **down and half-migrated** on an unmerged `.labby` branch; merged that branch up to date with main preserving all work (#175), fixed a self-inflicted `Cargo.lock` CI break (#176), deployed 0.30.0 to the container, and — after finding + fixing a `uv` provision bug (#177) — proved the full `incus-bootstrap.sh` → provision → running-service flow self-completes unattended with semantic search live.

## Sequence of Events

1. Inventoried gateway-only code, then inverted to a cut-list; wrote `docs/superpowers/plans/2026-07-02-base-service-feature-gating.md` (user removed docs-gen from the cut — registry-driven, documents the gateway itself).
2. work-it PR #171: gated stash/acp/nodes (bead lab-45uob); 3 review waves (lavra + PR-toolkit) → fix batches; the new gateway-slice nextest CI step exposed + fixed 7 pre-existing test failures.
3. work-it PR #172: codemode semantic search (bead lab-aq646); review wave found P1 unmetered `__lab_internal` amplification + response/query bounds → fixed (bead lab-7k11y). Both merged.
4. Deploy prep: found the Incus `labby` container down ~6.5h, half-migrated (`labby`/`.labby` vs `lab`/`.lab`), matching the unmerged `codex/incus-primary-deploy-clean-break` work. Surfaced the collision risk; user chose to merge that branch.
5. Merged `codex/incus-primary-deploy-clean-break` ↔ main (PR #175): 3 conflicts + 1 lint resolved keeping both sides; verified (2367 tests, lint, slices, docs).
6. Cargo.lock hotfix (#176): my merge didn't commit the regenerated lock, breaking every `--locked` CI job; fixed after a recovered `reset --hard` mishap.
7. Deployed 0.30.0 via `incus-bootstrap.sh --local-binary`; provision aborted at `uv-python`. Brought the gateway up manually + set `tei_url`.
8. Fixed the uv provision bug (#177: `cd $HOME` + `UV_NO_CONFIG=1`); rebuilt; re-ran the full bootstrap → self-completed unattended (service auto-started, semantic search live).

## Key Findings

- `main` provisioned `User=lab` / `/home/lab` / `.lab` (`dispatch/setup/host_service.rs:15,279`); the container ran `User=labby` / `/home/labby` / `.labby` from the *unmerged* clean-break branch — so main's binary couldn't even find the container's config (`~/.labby/` vs `~/.lab/`).
- The old 0.29.0 gateway crash-looped writing to a **read-only** `/home/labby/.lab/node-logs.sqlite` (`os error 30`); the `.labby` migration in 0.30.0 fixes exactly that.
- `uv python install` discovers config by walking up from CWD; provision ran from root's home so `uv` (as `labby` via `runuser`) hit `/root/uv.toml` (Permission denied), aborting before `systemctl enable --now` (`config/incus/labby-image.yaml` uv-python action).
- CI compiles with `RUSTFLAGS=-D warnings` (rustc lints); local `just lint` (clippy) misses `unused_qualifications`/`let_underscore_drop` — bit twice on #172 (saved to global memory).
- Squash-merged branches aren't ancestors of main, so `git merge-base --is-ancestor` can't prove them safe-to-delete.

## Technical Decisions

- Merged (not rebased/squashed) main into the clean-break branch to preserve every commit on both sides; resolved conflicts keeping main's `#[cfg(feature = "nodes")]` gating **with** the branch's `.labby` paths.
- Chose full bootstrap re-provision (once the `.labby` migration was merging into main, the earlier revert-collision risk dissolved).
- TEI URL = `http://100.64.0.79:52000` (dookie Tailscale IP; verified 200 from container; `127.0.0.1` confirmed unreachable from the container).
- Fixed uv centrally in the provision action (`cd $HOME` + `UV_NO_CONFIG=1`); siblings use absolute `$HOME` paths so no CWD fix needed there.

## Files Changed

| status | path | purpose |
|---|---|---|
| created | docs/superpowers/plans/2026-07-02-base-service-feature-gating.md | gating plan (#171) |
| modified | crates/labby/** (Cargo.toml, cli/api/dispatch/registry/node, tests) | stash/acp/nodes feature gating (#171) |
| modified | crates/labby-codemode/**, crates/labby-gateway/src/gateway/code_mode/**, labby-runtime/gateway_config.rs | semantic search (#172) |
| modified | crates/labby/src/cli/serve.rs, tests/nodes_master_only.rs, docs/services/STASH.md, crates/labby/src/cli/update.rs | #175 merge conflict/lint resolution |
| modified | Cargo.lock | labby-primitives 0.29.0→0.30.0 (#176) |
| modified | config/incus/labby-image.yaml | uv-python `cd $HOME` + UV_NO_CONFIG (#177) |
| created | docs/sessions/2026-07-02-{gate-base-services,codemode-semantic-search}-work-it.md | per-track logs (earlier this session) |
| created | docs/sessions/2026-07-02-incus-migration-deploy.md | this log |

## Beads Activity

| id | title | action | status |
|---|---|---|---|
| lab-45uob | gate base services | created/claimed/closed (impl) | closed |
| lab-c3x6u, lab-sdmbi | #171 review fix batches | created/closed | closed |
| lab-aq646 | codemode semantic search | created/closed | closed |
| lab-7k11y | codemode security fixes | created/closed | closed |
| lab-1x61l | uv-python provision bug | created, then **closed** (fixed by #177) | closed |
| lab-x4mw2 (+ siblings) | web-UI feature-unaware; structured 404; feature-table cleanup; env-schema warn | created as follow-ups | open (non-blocking) |

## Repository Maintenance

- **Plans**: the gating plan is complete but lives under `docs/superpowers/plans/` (not `docs/plans/`); not moved — the save-to-md commit is path-limited to this log, and a plan move belongs in a separate PR. Noted in Next Steps.
- **Beads**: closed lab-1x61l (uv fix verified live). Follow-up beads (lab-x4mw2 et al.) intentionally left open.
- **Worktrees/branches**: `feat/gate-base-services`, `fix/cargo-lock-0300`, `integrate/incus-clean-break`, `fix/provision-uv-config` are all PR-merged (squash) and safe to remove, but `git merge-base --is-ancestor` reports false due to squash. Left in place — several have live worktrees and the multi-worktree state includes another session's `codemode-wasmtime-dual-sandbox`. Cleanup deferred to avoid disruption; listed in Next Steps.
- **Stale docs**: PRs updated the affected docs in-band. One cosmetic leftover: `incus-bootstrap.sh` success text says `su - lab` instead of `su - labby` — not fixed (see Open Questions).

## Tools and Skills Used

- **Skills**: writing-plans, work-it, worktree-setup, lavra-review, review-pr, quick-push, save-to-md.
- **Agents**: ~2 implementation + ~2 fix + ~14 review agents across both PRs, background-dispatched with SendMessage steering.
- **Shell/git/gh/bd/incus/cargo** throughout. Issues: shell cwd resets between calls (worked around with `cd` prefixes); shared `target/` between worktrees caused contaminated builds + a foreign-binary smoke trap (`cargo clean -p` workaround); `--locked` CI vs non-locked local builds diverged (the #176 break); a `reset --hard` briefly discarded an uncommitted lock fix (recovered from the integrate worktree); direct push to main correctly blocked (used PRs); `bd create --type=improvement` invalid (used task/feature/bug).

## Commands Executed

| command | result |
|---|---|
| `cargo nextest run --workspace --all-features` (merge) | 2367 passed, 13 skipped |
| `cargo check --workspace --all-features --locked` (post-lockfix) | clean |
| `bash scripts/incus-bootstrap.sh --local-binary target/release-fast/labby --name labby` | exit 0; `provision complete: executed=1, skipped=8` |
| `incus exec labby -- systemctl is-active labby.service` | active (no manual restart) |
| `incus exec labby -- curl .../health` and `/ready` | 200 / 200 |
| `labby gateway code status --json` | `{tei_url: http://100.64.0.79:52000, blend_weight: 0.5}` |

## Errors Encountered

- Marketplace + Generated-docs CI (#171): missing `nodes` feature dep + stale feature-matrix → fixed.
- `unused_qualifications` / `let_underscore_drop` (#172 CI, `-D warnings`): two one-line fixes.
- `--locked` red across main (#176): Cargo.lock not committed after 0.30.0 bump → lock sync PR.
- `uv-python` provision abort (`/root/uv.toml` Permission denied): fixed in #177.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| full bootstrap re-run | exit 0, provision self-completes | exit 0, executed=1/skipped=8, service auto-started | pass |
| systemctl is-active labby.service | active (unattended) | active | pass |
| /health, /ready | 200 | 200, 200 | pass |
| gateway code status | tei_url set | `http://100.64.0.79:52000` | pass |
| `python --version` (labby) | resolves | Python 3.14.6 | pass |
| TEI reachable from container | 200 | 200 | pass |

## Risks and Rollback

- Container has snapshots (latest `labby-2026-07-02_02-37-18`); rollback via `incus restore` or `incus stop/delete` + re-bootstrap.
- The features are additive; with `tei_url` unset semantic search is inert, so a binary rollback leaves unconfigured behavior byte-identical.
- The 32-call internal-search ceiling (#172 hardening) silently degrades to lexical past the cap (visible via warn log).

## Open Questions

- `incus-bootstrap.sh` success message says `su - lab` (stale user name) instead of `su - labby` — cosmetic; fix on request.
- Post-#177 main CI rollup not re-confirmed fully green after auto-merge (gating checks passed; slow Incus/windows jobs may still be finishing).

## Next Steps

1. Optional: confirm main CI rollup green post-#177 (`gh pr checks` / `gh run list --branch main`).
2. Optional cleanup (separate PR/pass): delete the four merged branches + their worktrees once no session is using them; move the completed gating plan under a `complete/` location; fix the `su - lab` bootstrap text.
3. Follow-up beads (open, non-blocking): web-UI capability discovery, structured `feature_not_compiled` envelope for compiled-out routes, feature-table cleanup (`node-runtime`/`services-all`), env-schema parse-swallow warn.
