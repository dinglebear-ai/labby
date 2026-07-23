---
date: 2026-07-23 16:28:15 EDT
repo: git@github.com:dinglebear-ai/labby.git
branch: main
head: 4dc5ce628077ad71523ca9ece7b141222edba81e
session_id: 019f8d08-74a8-7b92-ab3f-0db4bba145ae
transcript: /home/jmagar/.codex/sessions/2026/07/22/rollout-2026-07-22T23-32-49-019f8d08-74a8-7b92-ab3f-0db4bba145ae.jsonl
working_directory: /home/jmagar/workspace/lab
primary_repo: /home/jmagar/workspace/labby
pull_requests:
  - https://github.com/dinglebear-ai/labby/pull/258
  - https://github.com/dinglebear-ai/incus-unraid/pull/12
beads:
  - lab-2s799
  - lab-x5g46
  - lab-5t4fr
  - lab-fw0si
  - lab-lb5ic
  - qs-kb4
  - qs-ibz
  - qs-dgq
  - qs-u3e
  - codex-full-review-20260718-dol
---

# Code Mode, Quick Shell, tootie SSH, and durable Labby OAuth repair

## User Request

The session began with a Code Mode MCP App resource failing to load with `-32603`. The request expanded into a connected set of product and runtime tasks:

1. Explain why the embedded app did not refresh automatically.
2. Simplify the Code Mode inspector: remove the `Find result` row, eliminate duplicate call and elapsed-time statistics, move the useful statistics into the first row, minimize the inspector by default beside a second MCP App, and remove the oversized read-only badge.
3. Explain the difference between `Calls` and `Request`.
4. Make Google `invalid_grant` failures surface as `oauth_needs_reauth`.
5. Mock up and then implement the chosen compact inspector treatment.
6. Restyle Quick Shell with Aurora tokens and connect it through Labby.
7. Replace opaque `MCP error -32000: MCP proxy request failed` failures with actionable copy and make Quick Shell actually attach.
8. Investigate repeated failed public-key authentication on tootie.
9. Fix Labby's Google sign-in flow, which returned users to `dinglebear.ai` instead of the Labby web app.
10. Remove the gitleaks workflow and baseline after CI required a paid organization license.
11. Preserve unrelated work because other agents and users were active in the repositories.

An unrelated request to create an `AGENTS.md` for an app-server working directory was explicitly withdrawn and was not performed.

## Session Overview

The work crossed three repositories and two live hosts. It produced working feature commits and two merged pull requests, but the final maintenance audit found that not every feature commit had reached the default branch.

- Labby's browser callback was durably separated from its OAuth issuer. PR [#258](https://github.com/dinglebear-ai/labby/pull/258) merged as `4dc5ce62`, the exact merged build was deployed, and the live login route now requests `https://labby.dinglebear.ai/auth/google/callback` while issuer metadata remains on `https://dinglebear.ai`.
- The gitleaks job, changed-path gate, baseline, and related documentation were removed in the same PR because the organization-level action required `GITLEAKS_LICENSE`.
- Unsafe Incus package artifacts were quarantined and verification was hardened in [incus-unraid#12](https://github.com/dinglebear-ai/incus-unraid/pull/12), merged as `71aa4b23`.
- The compact Code Mode inspector, private MCP App callbacks, `oauth_needs_reauth` mapping, and Quick Shell UI/bridge changes exist as local commits but are not ancestors of current Labby or Quick Shell `origin/main`. Follow-up beads now track landing them.

## Sequence of Events

1. The initial Code Mode resource read failed with an MCP `-32603`.
2. The stale embedded resource was traced to refresh/cache behavior rather than the resource implementation alone.
3. A compact inspector design was explored and mocked up. The selected direction consolidated the first two statistic rows, removed the search-result strip, defaulted to minimized presentation beside another MCP App, and reduced the read-only treatment.
4. The inspector implementation was committed as `ffaa2c3d`. A later auth commit was placed on the same local branch.
5. A delegated auth task implemented Google refresh-token classification so `invalid_grant` becomes `oauth_needs_reauth`, committed as `420f2d8b`.
6. Quick Shell was restyled with Aurora tokens in `67d5552`, then its Codex App bridge and session hydration were iterated in `6714c61`, `34b8858`, and `1ac89b4`.
7. Repeated `-32000` attachment failures showed that improving error wording alone did not repair the bridge. The generic proxy layer was inspected, then the app-specific bridge and deployed resource were changed.
8. Repeated tootie `sshd` failures were delegated for read-only diagnosis. The evidence led to unsafe Incus package directory modes rather than a bad user key.
9. The affected package artifacts were quarantined, mode-contract tests were added, tootie was repaired, and incus-unraid PR #12 was merged.
10. Labby's OAuth redirect was initially corrected in a local deployment, but a later deployment from `main` overwrote that fix. This exposed a source-control/delivery gap, not a second OAuth mechanism.
11. The durable OAuth change separated browser callback origin from issuer/resource origin and added regression coverage.
12. Gitleaks CI failed because the GitHub action required a paid license, not because it found a secret. At the user's direction the gitleaks surface was removed instead of adding a license.
13. Labby PR #258 merged, its exact binary was deployed into the live Incus container, and server-side redirect behavior was reverified.
14. The save-to-markdown maintenance pass closed the completed OAuth bead, removed only the merged OAuth worktree/branch, preserved unrelated worktrees, and created follow-ups for unlanded work.

## Key Findings

### Code Mode inspector

- `Calls` is an aggregate execution statistic: the number of MCP tool invocations represented by the run.
- `Request` is not a second call counter. It labels the expandable request/input payload for an individual call.
- The original display duplicated call count and elapsed time across the title and summary rows. The intended compact design keeps one status line and makes details expandable.
- The implementation commit exists, but it is not on Labby `main`.

### Quick Shell

- The user-facing `-32000` message was a proxy symptom. Rewording it made the failure more understandable but did not establish the missing nested app/session bridge.
- Direct capability hydration and app resource refresh changes were developed locally.
- Quick Shell `main` is clean but three commits ahead of its remote, while the Aurora command-bar UI remains on a separate branch. No pull request currently lands the complete set.

### tootie SSH

- The failed public-key messages were related to filesystem safety checks, not evidence that the authorized key itself had changed.
- Unsafe directory modes introduced by affected Incus package artifacts caused OpenSSH `StrictModes` rejection.
- The repair restored safe modes and package build 53 passed fresh, non-multiplexed SSH verification.

### Labby OAuth

- `LABBY_PUBLIC_URL=https://dinglebear.ai` is the public issuer/resource identity and should not have been changed merely to steer the browser back to the Labby UI.
- The browser callback needs its own externally reachable origin: `https://labby.dinglebear.ai/auth/google/callback`.
- The first working deployment was not durable because its commit was absent from `main`; a subsequent deployment legitimately replaced it.
- PR #258 made callback and issuer independent in source, provisioning, tests, and runtime documentation.

### Gitleaks

- CI failed at license validation for the organization-level gitleaks action.
- No secret finding caused the failure.
- The chosen resolution was complete removal of the job, route classification, baseline, and related documentation.

## Technical Decisions

- Keep OAuth issuer/resource identity on `dinglebear.ai`; introduce a distinct browser callback origin for the Labby web app.
- Treat `invalid_grant` as a reauthentication state, not an opaque internal MCP failure.
- Present Code Mode execution statistics once, with request and response bodies as expandable details.
- Default the inspector to minimized presentation when it accompanies another MCP App.
- Use Aurora's dark navy, cyan, rose, violet, semantic status, and monospace tokens for Quick Shell rather than ad hoc colors.
- Repair the actual Quick Shell capability/session bridge instead of relying on friendlier error copy.
- Trust tootie's live ownership/mode and journal evidence over the initial assumption that the SSH key was wrong.
- Quarantine unsafe package artifacts rather than silently leaving them installable.
- Remove gitleaks rather than creating or requesting a paid license secret.
- Preserve all worktrees and branches whose ownership or merge status was not proven.

## Files Changed

The table records the implementation work produced during the session, including work that remains on local branches.

| repo | status | path | purpose | landed state |
|---|---|---|---|---|
| labby | modified | `apps/gateway-admin/components/code-mode-app/code-mode-inspector.tsx` | Compact inspector layout and minimized behavior | local `ffaa2c3d` |
| labby | modified | `apps/gateway-admin/components/code-mode-app/code-mode-inspector.test.tsx` | Inspector regression coverage | local `ffaa2c3d` |
| labby | modified | `crates/labby/src/mcp/assets/code_mode_app.html` | Embedded inspector resource | local `ffaa2c3d` |
| labby | modified | `crates/labby/src/mcp/handlers_resources.rs` | Embedded resource version/serving behavior | local `ffaa2c3d` |
| labby | added | `design-qa.md` | Inspector visual QA record | local `ffaa2c3d` |
| labby | modified | `crates/labby-gateway/src/upstream/pool/tools.rs` | Expose private MCP App callbacks | local `2e6963de` |
| labby | modified | `crates/labby-auth/src/error.rs` | Reauthentication error classification | local `420f2d8b` |
| labby | modified | `crates/labby-auth/src/google.rs` | Map Google `invalid_grant` | local `420f2d8b` |
| labby | modified | `crates/labby-auth/src/token.rs` | Propagate refresh reauthentication state | local `420f2d8b` |
| labby | modified | `docs/dev/ERRORS.md` | Document `oauth_needs_reauth` | local `420f2d8b` |
| labby | modified | `docs/runtime/OAUTH.md` | Reauth and callback documentation | local `420f2d8b`, also changed in merged PR |
| labby | modified | `.github/CLAUDE.md` | Remove obsolete gitleaks guidance | merged `4dc5ce62` |
| labby | modified | `.github/workflows/ci.yml` | Remove gitleaks job and dependencies | merged `4dc5ce62` |
| labby | deleted | `.gitleaksignore` | Remove gitleaks baseline | merged `4dc5ce62` |
| labby | modified | `crates/labby-auth/src/authorize.rs` | Build browser authorization callback independently | merged `4dc5ce62` |
| labby | modified | `crates/labby-auth/src/config.rs` | Add callback-origin configuration | merged `4dc5ce62` |
| labby | modified | `crates/labby-auth/src/metadata.rs` | Preserve issuer metadata identity | merged `4dc5ce62` |
| labby | modified | `crates/labby-auth/src/state.rs` | Carry callback configuration | merged `4dc5ce62` |
| labby | modified | `crates/labby/src/api/router.rs` | Wire callback configuration into routes | merged `4dc5ce62` |
| labby | modified | `crates/labby/src/api/upstream_oauth.rs` | Keep upstream OAuth callback behavior consistent | merged `4dc5ce62` |
| labby | modified | `crates/labby/src/dispatch/setup/host_service.rs` | Provision host-service callback environment | merged `4dc5ce62` |
| labby | modified | `crates/labby/src/dispatch/setup/provision.rs` | Provision callback configuration | merged `4dc5ce62` |
| labby | modified | `crates/labby/tests/auth_admin_api.rs` | Regression-test separate callback and issuer | merged `4dc5ce62` |
| labby | modified | `crates/labby/tests/ci_changed_paths.rs` | Remove gitleaks routing expectations | merged `4dc5ce62` |
| labby | modified | `docs/runtime/CICD.md` | Remove gitleaks CI instructions | merged `4dc5ce62` |
| labby | modified | `docs/runtime/CONFIG.md` | Document callback configuration | merged `4dc5ce62` |
| labby | modified | `docs/runtime/ENV.md` | Document callback environment | merged `4dc5ce62` |
| labby | modified | `scripts/ci/changed_paths.py` | Remove gitleaks job classification | merged `4dc5ce62` |
| quick-shell | added | `design-qa.md` | Aurora command-bar visual QA | local `67d5552` |
| quick-shell | modified | `src/app/styles.css` | Aurora token styling and compact app chrome | local `67d5552` |
| quick-shell | modified | `src/app/view.ts` | Aurora command-bar UI | local `67d5552` |
| quick-shell | modified | `src/app/mcp-app.ts` | Codex App bridge and direct session hydration | local `6714c61`, `1ac89b4` |
| quick-shell | modified | `src/server/create-server.ts` | Refresh resource/session server behavior | local `34b8858`, `1ac89b4` |
| quick-shell | modified | `src/server/mcp-tooling.ts` | Hydrate session tool/capability metadata | local `34b8858`, `1ac89b4` |
| quick-shell | modified | `tests/app/mcp-app.test.ts` | App bridge and UI regression coverage | local commits |
| quick-shell | modified | `tests/server/create-server.test.ts` | Server resource/session regression coverage | local commits |
| incus-unraid | added | `packages/quarantine/unsafe-root-directory-mode/README.md` | Explain unsafe package quarantine | merged `71aa4b23` |
| incus-unraid | moved | `packages/incus-unraid-7.0.0-48-x86_64-1.txz` | Quarantine unsafe build 48 | merged `71aa4b23` |
| incus-unraid | moved | `packages/incus-unraid-7.0.0-49-x86_64-1.txz` | Quarantine unsafe build 49 | merged `71aa4b23` |
| incus-unraid | moved | `packages/incus-unraid-7.0.0-50-x86_64-1.txz` | Quarantine unsafe build 50 | merged `71aa4b23` |
| incus-unraid | moved | `packages/incus-unraid-7.0.0-51-x86_64-1.txz` | Quarantine unsafe build 51 | merged `71aa4b23` |
| incus-unraid | modified | `scripts/verify-classic-package.sh` | Reject unsafe package directory modes | merged `71aa4b23` |
| incus-unraid | modified | `tests/classic-contract.sh` | Extend package contract coverage | merged `71aa4b23` |
| incus-unraid | added | `tests/package-directory-modes.sh` | Mode-specific package regression test | merged `71aa4b23` |
| labby | added | `docs/sessions/2026-07-23-code-mode-quick-shell-tootie-oauth-debugging.md` | Complete session and maintenance record | this artifact |

## Beads Activity

| bead | repository | activity | final state |
|---|---|---|---|
| `lab-2s799` | labby | Tracked compact Code Mode inspector implementation | closed on local branch; not landed |
| `lab-x5g46` | labby | Tracked private MCP App callback exposure | closed on local branch; not landed |
| `lab-5t4fr` | labby | Tracked actionable MCP proxy failure detail | closed after app-boundary handling |
| `lab-fw0si` | labby | Tracked separate Google browser callback origin | closed after PR #258 merge and exact-build deployment |
| `lab-lb5ic` | labby | Created during maintenance for all unlanded Labby feature commits | open, priority 1 |
| `qs-kb4` | quick-shell | Wrong-repository inspector task | closed as superseded |
| `qs-ibz` | quick-shell | Tracked Aurora command-bar UI | closed on local branch; not published |
| `qs-dgq` | quick-shell | Tracked direct capability hydration | closed on local commits; not published |
| `qs-u3e` | quick-shell | Created during maintenance to consolidate and publish Quick Shell changes | open, priority 1 |
| `codex-full-review-20260718-dol` | incus-unraid | Tracked unsafe-build quarantine and tootie remediation | closed after PR #12 and live verification |

## Repository Maintenance

- Plans: `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already complete. `docs/plans/fleet-ws-plan-lab-n07n.md` still contains substantial unchecked work and remained active.
- Beads: closed `lab-fw0si` with merge/deployment evidence; created `lab-lb5ic` and `qs-u3e` for work proven not to be on the default branches.
- Worktrees: removed the clean, merged `/home/jmagar/.codex/worktrees/labby/durable-browser-callback` worktree.
- Branches: deleted local and remote `codex/durable-browser-callback` only after confirming PR #258 was merged. Unmerged, shared, or unknown branches and worktrees were preserved.
- Stale documentation: current OAuth, environment, configuration, and CI docs were updated by PR #258. Historical session and changelog references to gitleaks were retained as history.
- Primary checkout: no reset, pull, or cleanup was applied to the shared `/home/jmagar/workspace/labby` checkout.

## Agents, Tools, and Skills Used

- `superpowers:systematic-debugging` structured the failing-layer investigations.
- `superpowers:using-git-worktrees`, `superpowers:test-driven-development`, and `superpowers:verification-before-completion` supported isolated implementation and evidence-based delivery.
- `labby:using-labby` supported live gateway and runtime checks.
- `github:gh-fix-ci` supported GitHub Actions diagnosis.
- `vibin:save-to-md` drove this full transcript and maintenance record.
- A subagent implemented the `invalid_grant` to `oauth_needs_reauth` behavior.
- A subagent performed read-only tootie SSH investigation.
- Shell and repository tools included `rg`, Git, GitHub CLI, `cargo`, `pnpm`, `just`, `curl`, `jq`, `systemctl`, `journalctl`, SSH, Incus, `sha256sum`, and archive inspection.
- MCP/App surfaces included Labby gateway search/execute, Code Mode, Quick Shell resources, and embedded Codex App state.

## Commands Executed

| command or class | result |
|---|---|
| `rg` over app, auth, proxy, resource, and CI code | Located duplicated inspector state, proxy error boundaries, OAuth callback construction, and gitleaks routing |
| `git log`, `git show`, `git diff`, `git merge-base --is-ancestor` | Proved which commits were merged and which remained local |
| Targeted inspector UI tests | 38 tests passed during inspector work |
| Gateway/Rust test suites | 541 gateway tests passed during inspector work |
| `cargo test -p labby-auth` | 68 tests passed after durable callback changes |
| `just web-build` | Next.js build passed; 57 pages and route bundle checks completed |
| `just build` | All-features release-fast Labby build completed |
| `labby incus sync ...` and runtime polling | Deployed exact merged Labby binary and confirmed readiness |
| `curl` against `/auth/login`, metadata, and `/ready` | Callback host is Labby; issuer/token endpoints remain dinglebear; ready is true |
| `actionlint` and CI routing tests | Workflow syntax passed; 14 routing tests passed after gitleaks removal |
| Fresh non-multiplexed SSH to tootie | Public-key authentication passed after mode repair |
| Incus package verification and contract tests | Safe build 53 and package-mode protections passed |
| GitHub PR/check commands | incus-unraid #12 and Labby #258 merged with checks |

## Errors Encountered

| error | cause | resolution or state |
|---|---|---|
| MCP resource read `-32603` | Embedded app/resource refresh state was stale | Resource version/serving changes developed locally; complete feature still needs landing |
| Inspector did not auto-refresh | Already-running app/gateway state did not reload solely from source/config changes | Rebuilt/refreshed resource and restarted relevant runtime during development |
| Quick Shell `MCP -32000` | Generic proxy error hid a nested app/session bridge failure | Improved app-specific handling and developed direct hydration; publication remains open |
| tootie `Failed publickey` | Unsafe filesystem modes triggered OpenSSH `StrictModes` rejection | Restored safe modes, quarantined affected packages, added tests |
| OAuth returned to `dinglebear.ai` | Browser callback was derived from issuer identity | Added independent browser callback configuration |
| OAuth fix disappeared after deployment | First fix was only in a local build, not default-branch source | Merged PR #258 and deployed the exact merged binary |
| `next: not found` in clean worktree | App dependencies were not installed in that worktree | Installed from `apps/gateway-admin` using its lockfile |
| `pnpm install --frozen-lockfile` failed at repository root | The relevant lockfile is app-local | Ran install from the correct application directory |
| First targeted Rust regression ran zero tests | `--exact` did not match the generated test name | Reran without `--exact` and observed the expected failing test |
| `cargo fmt --check` failed | Two pre-existing files on main were unformatted | Applied mechanical formatting before delivery |
| Gitleaks Action failed | Organization action required paid `GITLEAKS_LICENSE` | Removed gitleaks CI and baseline at user direction |
| `gh pr merge --delete-branch` reported local-main ownership | Another worktree owned local `main` | Merge itself succeeded; branch cleanup was completed separately and safely |
| Build wrapper returned before child exit | `soldr`/build wrapper detached from the active child | Polled process completion and verified artifact hash |

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| OAuth browser callback | Derived from issuer and returned to `dinglebear.ai` | Live login requests `labby.dinglebear.ai/auth/google/callback` |
| OAuth issuer metadata | `dinglebear.ai` | Still `dinglebear.ai` |
| Gitleaks CI | License-gated job blocked CI | Job, route, baseline, and current docs removed |
| tootie SSH | Valid key rejected by StrictModes | Fresh public-key SSH succeeds with safe modes |
| Incus packages | Unsafe builds remained in normal package path | Builds 48–51 quarantined and mode tests enforce safety |
| Code Mode inspector | Repeated stats, search-result row, large read-only treatment | Compact implementation exists locally; not yet on main |
| Quick Shell | Generic attach failure and older UI | Aurora/bridge fixes exist locally; not yet on origin/main |
| `invalid_grant` | Could surface as opaque MCP failure | `oauth_needs_reauth` implementation exists locally; not yet on main |

## Verification Evidence

| target | expected | actual | status |
|---|---|---|---|
| Labby auth tests | Callback separation covered | 68 passed | pass |
| Labby web build | Embedded UI compiles | 57 pages built and bundle checks passed | pass |
| Labby release build | Deployable binary | all-features release-fast build completed | pass |
| Live binary identity | Runtime equals merged artifact | SHA-256 `7648d65f28950874068fcee7ab57c42442da610ba967a34dfa87b33ce11ac401` matched | pass |
| Live startup config | Separate callback and issuer | `public_url=https://dinglebear.ai/`; Google callback uses Labby host | pass |
| Live `/auth/login` | Google redirect URI uses Labby | `https://labby.dinglebear.ai/auth/google/callback` | pass |
| OAuth metadata | Issuer endpoints stay stable | issuer, authorization, and token endpoints remain dinglebear | pass |
| Live readiness | Service ready | `/ready` returned ready | pass |
| CI after gitleaks removal | Workflow valid and routing correct | actionlint passed; 14 tests passed | pass |
| tootie SSH | Fresh key authentication works | passed without multiplexing | pass |
| Incus package protection | Unsafe modes rejected | package and live build verification passed | pass |
| Labby remaining feature commits | Ancestor of `origin/main` | `ffaa2c3d`, `2e6963de`, and `420f2d8b` are not ancestors | open |
| Quick Shell publication | Changes on `origin/main` | three commits ahead locally; Aurora UI separate | open |

The server-side OAuth callback contract is verified. A complete end-user Google consent/login completion after the final deployment was not observed in this session.

## Risks and Rollback

- Reverting all of `4dc5ce62` would recouple browser callback and issuer and would also reintroduce the gitleaks surface. Prefer a focused follow-up or targeted revert if a specific regression appears.
- Quick Shell and the compact inspector are at risk of local-branch drift until their follow-up beads are completed.
- Closed beads for local-only work can look delivered when viewed without Git ancestry. The new open beads are the authoritative remaining-work signal.
- Unsafe package artifacts should remain quarantined. Restoring them to the normal package path can recreate tootie-wide permission failures.
- Shared worktrees contain other work. Cleanup must continue to target only branches whose ownership and merge state are proven.

## Decisions Not Taken

- Did not change `LABBY_PUBLIC_URL` to the Labby hostname because that value defines issuer/resource identity.
- Did not claim that friendlier `-32000` text fixed Quick Shell.
- Did not merge the inspector, private callback, reauth, or Quick Shell feature branches opportunistically during OAuth recovery.
- Did not reset, pull, or clean the shared primary checkout.
- Did not delete worktrees with unmerged or unknown ownership.
- Did not add a gitleaks license secret.
- Did not remove historical documentation merely because it mentions gitleaks.
- Did not create the withdrawn `AGENTS.md`.

## Open Questions

- Does a real interactive Google login now complete all the way back into the Labby web app? The redirect contract is correct, but the final browser completion still needs a human login attempt.
- Which portions of `ffaa2c3d`, `2e6963de`, and `420f2d8b` need rebasing or adaptation to current `main` before one focused Labby PR?
- Should the Quick Shell Aurora UI and three bridge commits be squashed, rebased, or retained as separate commits when publishing?

## Next Steps

### Unfinished work

1. Complete `lab-lb5ic`: rebase and review the compact inspector, private MCP App callback, and `oauth_needs_reauth` work; run targeted UI/Rust tests; merge; rebuild embedded assets; deploy the exact merged commit; verify Code Mode and Quick Shell live.
2. Complete `qs-u3e`: consolidate `67d5552`, `6714c61`, `34b8858`, and `1ac89b4`; run the full Quick Shell suite; publish through a focused PR; deploy exact artifacts; verify `open_quick_shell` attach and output.
3. Perform one real Google sign-in at `labby.dinglebear.ai`. If it fails, capture the callback URL and the corresponding Labby journal entries before changing configuration.

### Follow-on improvements

1. Make the inspector show a concise distinction between aggregate `Calls` and the selected call's `Request` payload.
2. Add delivery checks that compare the deployed build/manifest commit to the merged default-branch commit before declaring a UI or MCP App fix live.
3. Consider a cross-repository release checklist for Labby-hosted MCP Apps so gateway, embedded resource, app server, and live artifact versions cannot drift silently.

## References

- Labby PR [#258: Separate browser callback from issuer](https://github.com/dinglebear-ai/labby/pull/258)
- incus-unraid PR [#12: Quarantine unsafe Incus package builds](https://github.com/dinglebear-ai/incus-unraid/pull/12)
- Labby follow-up bead `lab-lb5ic`
- Quick Shell follow-up bead `qs-u3e`
- Session transcript: `/home/jmagar/.codex/sessions/2026/07/22/rollout-2026-07-22T23-32-49-019f8d08-74a8-7b92-ab3f-0db4bba145ae.jsonl`
