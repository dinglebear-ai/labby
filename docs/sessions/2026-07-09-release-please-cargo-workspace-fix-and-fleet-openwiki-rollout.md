---
date: 2026-07-09 00:46:47 EST
repo: git@github.com:jmagar/labby.git
branch: main
head: 45c2079f
working directory: /home/jmagar/workspace/lab
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/8775dbe1-467e-4d07-b845-adfea8cfb858.jsonl
---

# release-please cargo-workspace fix and fleet OpenWiki rollout

## User Request

Session started with "tell me the repo status" for `lab`, escalated through several
follow-ups: investigate `release-please`/OpenWiki CI failures and fix them, roll the
OpenWiki proxy fix out fleet-wide plus write a batch of missing operator docs, reconcile
dotfiles drift, fix a dead GitHub Actions self-hosted runner on `steamy`, and finally
resolve `release-please`'s repeated failure, merge the resulting release, and sync the
built binary into the local PATH and the Incus gateway container.

## Session Overview

A single-repo CI investigation (`lab`'s `release-please` and `OpenWiki` workflows both
failing) grew into: (1) a fleet-wide rollout of the OpenWiki fix across ~20 repos plus 12
new operator docs in `~/docs`, (2) a full chezmoi dotfiles reconciliation including
retiring `.rustarr`/`.lab` in favor of `.yarr`/`.labby`, (3) diagnosing and fixing a
self-hosted GitHub Actions runner on `steamy` that was dying whenever its console window
closed, and (4) a deep, verified-against-source-code fix for `release-please`'s
incompatibility with this repo's Cargo workspace structure, ending in a real `v1.0.0`
release built, tagged, and synced onto the local machine and the Incus container.

## Sequence of Events

1. **Initial repo status check** — `lab` clean on `main`, but `release-please` and
   `OpenWiki Update` workflows both red on recent pushes.
2. **Diagnosed `release-please` failure #1** — `.release-please-manifest.json` pointed at
   `0.30.0`, a version set in `Cargo.toml`/`CHANGELOG.md` before `release-please` existed
   and never tagged. Fixed by resetting the manifest to `0.29.0` (the real last tag).
3. **Diagnosed `OpenWiki` failure** — z.ai Anthropic-compatible endpoint hitting its
   5-hour rate limit. User provided a replacement `openai-compatible` proxy
   (`cli-api.tootie.tv`, `gpt-5.3-codex-spark`) and asked to switch to it.
4. **User requested a fleet-wide rollout** — find every repo using OpenWiki and/or
   `release-please`, migrate all to the new proxy, backfill OpenWiki into repos that had
   `release-please` but not OpenWiki, write ~12 new operator docs (OpenWiki, beads, lavra,
   node-entrypoint packaging, MCP registry submission, CI path-gating, xtask/mcporter live
   testing, xtask patterns, agent-first service patterns, `marketplace-no-mcp` branch
   sync, new-repo-setup checklist), build a combined `.gitignore` reference, and SSH into
   `squirts` to document the remote Dolt server backing `beads`.
5. **Dispatched 9 parallel research agents** (OpenWiki+beads/Dolt, squirts SSH review,
   lavra, yarr-rmcp node packaging + xtask/mcporter, template-rmcp xtask patterns +
   other patterns, axon CI path-gating, MCP registry docs, no-mcp branch sync x2) while
   directly executing the mechanical rollout (14 direct-push repos, `axon` via
   [PR #390](https://github.com/jmagar/axon/pull/390) since its `main` is protected,
   `memos` skipped — its `origin` is upstream `usememos/memos`, not the user's fork).
6. **Wrote all 12 docs + combined `.gitignore`** into `~/docs`, captured via
   `chezmoi add`/`chezmoi re-add` (auto-commits + auto-pushes to `jmagar/dotfiles`).
7. **User created `~/docs/.env`** with shared secret values (`GITHUB_TOKEN`,
   OpenWiki proxy creds) and asked docs updated to reference it. Updated `openwiki.md`
   and `new-repo-setup.md` accordingly; verified `~/docs/.env` stays chezmoi-unmanaged.
8. **User asked to add `BEADS_DOLT_PASSWORD`** — a redaction-regex mistake briefly
   printed the user's `PYPI_TOKEN` in cleartext into the conversation; flagged
   immediately and the user was advised to rotate it. `BEADS_DOLT_PASSWORD` was then
   added safely (piped directly from the live shell env, never printed).
9. **Full chezmoi dotfiles reconciliation** — inspected drift across ~13 files
   (`.cargo/config.toml`, `.ssh/config`, `.npmrc`, `.codex/.credentials.json`, etc.),
   confirmed live-newer-than-source via mtime before `re-add`ing each, skipped
   `fzf-git.sh` (externally pinned, not a re-add candidate), ran two pending
   `run_onchange` scripts manually, left one cosmetic `.rustarr` permission-mode
   mismatch unresolved (flagged, not fixed).
10. **User retired `.rustarr`/`.lab`, added `.yarr`/`.labby`** — `chezmoi forget`
    (unmanage, not delete) on the old dirs, `chezmoi add --encrypt` on the new dirs'
    config/`.env` files only (not the full multi-hundred-MB runtime directories).
11. **User asked for repo status again** — CI on `lab` was stuck `queued` for hours;
    root cause traced to the self-hosted runner `steamy-lab` being offline.
12. **Investigated `steamy` via `steamy-windows-mcp`** — initial attempt hit an RPC
    error (machine unreachable via GUI automation); switched to SSH into `steamy-wsl`
    with `powershell.exe` interop instead.
13. **Found and fixed the runner root cause** — `labby-actions-runner.lnk` launched
    `run.cmd` directly in a visible (minimized) console window with no supervision;
    its own diagnostic log showed the connection aborted when that window was closed.
    Replaced it with a hidden VBScript launcher (`WshShell.Run(path, 0, False)`) wrapping
    a new auto-restart `run-loop.cmd`, mirroring the working pattern already used for the
    `rmcp-template` runner. Started it immediately; confirmed online via
    `gh api repos/jmagar/labby/actions/runners`.
14. **CI ran and failed** on a pre-existing `cargo fmt` drift in `crates/labby/src/config.rs`
    (unrelated to the OpenWiki/manifest fix); fixed and pushed (`14716468`).
15. **`release-please` failed again** with the same `"value at path package.version is
    not tagged"` error despite the manifest fix. Root-caused by testing release-please's
    updater classes directly against the library source with no network calls: the
    `rust` release-type's `CargoToml` updater unconditionally tries to write a scalar into
    `[package].version`, which throws for this workspace's structure — the root
    `Cargo.toml` is a pure virtual workspace manifest (no `[package]` table) and every
    member crate uses `version.workspace = true` (inherited, no literal version string).
16. **Presented the finding and three options to the user**; user chose
    `release-type: "simple"` + a follow-up `sync-cargo-version` job.
17. **Validated the fix locally before touching CI again** — used `release-please`'s own
    `manifest-pr --dry-run` CLI. Discovered (the hard way, twice) that `--local
    --local-path=<dir>` silently resets *uncommitted* changes in the target directory to
    its last commit, and separately that it still fetches real file content from the
    GitHub API for the target branch regardless of `--local` — meaning local-only edits
    were never actually exercised. Worked around this by pushing a real throwaway branch
    (`test/release-please-simple-type`) and dry-running against that, which correctly
    reproduced the fix's behavior with zero live-branch risk.
18. **Implemented and pushed the real fix** (`8a3f5809`): `release-type: "simple"` in
    `release-please-config.json`, plus a new `sync-cargo-version` job in
    `release-please.yml` that patches `[workspace.package].version` in `Cargo.toml` and
    regenerates `Cargo.lock` via `cargo check` on the release PR branch after
    `release-please` opens/updates it.
19. **Watched the fix work end-to-end** — CI passed, `release-please` opened
    [PR #198](https://github.com/jmagar/labby/pull/198) "chore(main): release 1.0.0"
    (major bump legitimate — a breaking-change commit was in the unreleased range),
    the `sync-cargo-version` job ran successfully, and the diff was verified correct
    (only `[workspace.package].version` changed in root `Cargo.toml`, 12 matching
    `Cargo.lock` version bumps, no individual crate `Cargo.toml` touched).
20. **User asked to merge** — merged `#198`; watched the merge-commit CI run,
    `release-please` cut the actual `v1.0.0` tag/GitHub Release, `release.yml` started
    building.
21. **`release.yml` failed** on `gh release edit --generate-notes` — an invalid flag
    combo (`--generate-notes` is `create`-only). Fixed and pushed (`45c2079f`) for future
    releases; manually downloaded the already-built artifacts from the failed run and
    attached them to the existing `v1.0.0` release via `gh release upload`.
22. **User asked to install the binary and sync it to Incus** — ran
    `labby update --version v1.0.0` (dry-run first), which downloaded, sha256-verified,
    installed to `~/.local/bin/labby`, and synced into the Incus gateway container in one
    step. Confirmed both report `labby 1.0.0`.

## Key Findings

- `release-please`'s `CargoToml` updater (`updaters/rust/cargo-toml.js`) unconditionally
  runs `replaceTomlValue(payload, ['package', 'version'], ...)` — verified directly
  against the installed library source with an isolated Node script, no network calls.
  This throws two distinct errors for this repo's structure: `"is not a package manifest
  (might be a cargo workspace)"` for the virtual root `Cargo.toml` (no `[package]` table),
  and `"value at path package.version is not tagged"` for every member crate (
  `version.workspace = true`, no literal scalar to overwrite). Neither is configurable
  away via `release-type`, `extra-files`, or `plugins` — confirmed the `extra-files`
  `jsonpath` config for a file literally named `Cargo.toml`/`Cargo.lock` is silently
  overridden by the hardcoded Cargo-aware updater classes.
- `release-please manifest-pr --local --local-path=<dir>` resets uncommitted changes in
  the target directory to its last git commit before running, and even then still fetches
  actual file *content* from the GitHub REST/GraphQL API for the configured branch rather
  than reading local files — `--local` only affects where commit-history parsing happens,
  not file content. The only reliable way to dry-run a config change is against a real
  pushed branch.
- `crates/labby/src/config.rs:16` had pre-existing `cargo fmt` drift on `main`, unrelated
  to any change in this session — caught by CI on the first push after the runner came
  back online.
- The self-hosted runner `steamy-lab` (`jmagar/labby`) died on 2026-07-06 when its
  console window was closed — confirmed via its own `_diag/Runner_*.log`
  (`System.Threading.Tasks.TaskCanceledException` / "Runner execution been cancelled").
  It had no supervision/auto-restart, unlike the working `rmcp-template` runner on the
  same machine, which uses a hidden VBScript + `run-loop.cmd` pattern.
- `~/.rustarr` had genuinely diverged live-vs-source permission bits (`750`/`640` live vs.
  chezmoi's declared `private_` target of `700`/`644`) — likely intentional for a
  systemd service's group-read access. Left unresolved rather than guessed at (now moot,
  `.rustarr` was subsequently retired by the user in favor of `.yarr`).

## Technical Decisions

- Chose `release-type: "simple"` + a follow-up `sync-cargo-version` CI job over (a)
  manually bumping `Cargo.toml` on every release or (b) restructuring the workspace to
  use explicit per-crate versions — preserves both full release automation and the
  repo's existing `version.workspace = true` convention, at the cost of one extra CI job.
  Explicitly presented as a user decision (`AskUserQuestion`) given it's a real release-
  pipeline architecture change, not a one-line fix.
- Validated the `Cargo.toml` version-patch regex against the real file content before
  trusting it in CI, and validated the `release-please` config change against a real
  pushed throwaway branch (not local files) before pushing to `main`, after two prior
  guesses had already cost real CI cycles.
- Did not force through the `.rustarr` permission-mode diff during chezmoi reconciliation
  — applying it would have loosened `Cargo.lock`-adjacent-secret-file permissions
  (`640→644`) without understanding why the live value was more restrictive.
- Skipped `memos` in the OpenWiki fleet rollout rather than guessing at a fork remote —
  its `origin` is the upstream `usememos/memos` OSS project, not the user's own repo.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| modified | `.release-please-manifest.json` | Reset stale version (`0.30.0`→`0.29.0`), later auto-bumped by release-please to `1.0.0` on release | commits `9bef3f0f`, `603ef4de` |
| modified | `.github/workflows/openwiki-update.yml` | Switched OpenWiki provider to `openai-compatible` proxy (`cli-api.tootie.tv`) | commit `9bef3f0f` |
| modified | `crates/labby/src/config.rs` | Fixed pre-existing `cargo fmt` drift (import ordering) | commit `14716468` |
| modified | `release-please-config.json` | `release-type: "rust"` → `"simple"` | commit `8a3f5809` |
| modified | `.github/workflows/release-please.yml` | Added `sync-cargo-version` job; rewrote header comment explaining the cargo-workspace incompatibility | commit `8a3f5809` |
| modified | `Cargo.toml` | `[workspace.package].version` `0.30.0`→`1.0.0`, auto-committed by the new sync job | commit `2c4e2570` (PR #198) |
| modified | `Cargo.lock` | 12 matching version-field bumps via `cargo check`, auto-committed by the new sync job | commit `2c4e2570` (PR #198) |
| modified | `CHANGELOG.md` | Auto-generated release notes for `v1.0.0` | commit `603ef4de` (PR #198) |
| modified | `.github/workflows/release.yml` | Removed invalid `gh release edit --generate-notes` flag | commit `45c2079f` |

Cross-repo work performed in the same session but outside this repo's tree (not reflected
in the table above; see the fleet-rollout tracker for the authoritative list):
`~/docs/dev/{openwiki,beads,lavra,node-entrypoint-rust-mcp,ci-path-gating,
xtask-mcporter-live-testing,xtask-patterns,agent-first-service-patterns,
no-mcp-branch-sync,new-repo-setup,megatask-2026-07-08-openwiki-docs-rollout}.md`,
`~/docs/mcp/registry-submission.md`, `~/docs/.gitignore`, `~/docs/.env` (secrets, not
committed anywhere), OpenWiki workflow updates across 19 other repos, `axon`
[PR #390](https://github.com/jmagar/axon/pull/390), chezmoi dotfiles re-adds across ~15
files in `jmagar/dotfiles`, and the `steamy` runner launcher swap
(`labby-actions-runner.lnk` → `labby-actions-runner.vbs` + `run-loop.cmd`).

## Beads Activity

No bead activity observed. This session's work (CI/release infrastructure debugging,
fleet documentation rollout, dotfiles reconciliation, runner recovery) was not tracked
against an existing bead, and no new bead was created — all identified work was
completed and verified within the session itself (release shipped, runner confirmed
online, docs written and pushed), leaving no dangling follow-up that clearly warranted
its own tracked issue.

## Repository Maintenance

- **Plans**: `docs/plans/fleet-ws-plan-lab-n07n.md` reviewed — open, unrelated to this
  session's work, left in place. No plans were completed or moved this session.
- **Beads**: none touched; see above.
- **Worktrees and branches**: created and fully cleaned up one throwaway branch
  (`test/release-please-simple-type`, deleted both remote and local after use). Did not
  touch the four `.claude/worktrees/*` agent worktrees present
  (`elated-khorana-881639`, `great-wiles-864b4c`, `unruffled-keller-bce807`,
  `vigilant-solomon-0ffa4b`) or the protected `marketplace-no-mcp` worktree — none were
  created by this session and their ownership/in-progress status is unknown.
- **Stale docs**: rewrote the `release-please.yml` header comment in place to describe
  the new `simple`+`sync-cargo-version` mechanism (the old comment described the
  now-incorrect `rust` release-type behavior). Noted but did **not** fix:
  `.github/CLAUDE.md`'s "Release Process" section still describes a `cargo-release`-based
  flow that doesn't match the actual `release-please`-based mechanism in this repo — this
  predates the session and is out of scope for this pass; flagged in Next Steps.
- **Transparency**: the dirty files reported in this session's injected git-status
  context (`apps/gateway-admin/*`, `crates/labby-gateway/*`, `crates/labby-runtime/*`,
  `crates/labby/src/mcp/*`, `docs/generated/*`) were **not** touched by this session —
  pre-existing uncommitted work from elsewhere, left untouched.

## Tools and Skills Used

- **Shell (Bash)**: git, gh CLI, cargo, npx/node, chezmoi, ssh, scp, python3 — the
  overwhelming majority of the session's work.
- **File tools (Read/Edit/Write)**: config/workflow edits across `lab` and doc authoring
  under `~/docs`.
- **Agent tool (parallel research)**: 9 concurrent research agents dispatched for the
  fleet-documentation task (OpenWiki/beads/Dolt, squirts SSH, lavra, yarr-rmcp packaging
  + testing, template-rmcp xtask + patterns, axon CI gating, MCP registry docs, no-mcp
  branch sync ×2). All completed successfully; no failures or retries needed.
- **`computer-use` MCP**: attempted first for the `steamy` runner investigation; the user
  denied the access-request dialog, so this path was abandoned in favor of SSH.
- **`steamy-windows-mcp` (via Labby `codemode`)**: attempted next; returned
  `"RPC server is unavailable"` (machine unreachable for GUI automation at that moment,
  not an approval-gate issue as initially suspected). Abandoned in favor of SSH.
- **SSH (`steamy-wsl`) + `powershell.exe` WSL interop**: successful path for all `steamy`
  investigation and remediation (Get-Service, process listing, Startup-folder inspection,
  shortcut/VBS content reads and writes). One `scp` invocation failed on a
  space-containing remote path (`ambiguous target`); worked around with
  `ssh ... "cat > '<path>'" < localfile`.
- **`Monitor` tool**: used repeatedly to watch CI/`release-please`/`release.yml` runs
  asynchronously. Two invocations crashed on setup bugs in this session — one used a
  read-only zsh variable name (`status`), one had a bare `jq` call with no error
  tolerance — both fixed and restarted successfully on retry.
- **`WebFetch`**: used to pull `release-please` documentation and (via `gh api`, since
  raw `WebFetch` hit a 429) to inspect `release-please`/`release-please-action` source
  files directly.
- **`AskUserQuestion`**: used three times — steamy investigation redirect
  (computer-use → SSH), the `release-please` architecture-decision fix, and the
  `~/docs/.env` key additions (`MCP_PRIVATE_KEY` scoping).
- **`chezmoi`**: `add`, `re-add`, `forget`, `status`, `diff`, `managed`, `source-path`,
  `trust` — the primary tool for the dotfiles-reconciliation portion of the session.

## Commands Executed

| command | result |
|---|---|
| `git log --oneline -1 -S'version = "0.30.0"' -- Cargo.toml` | Traced the untagged-version root cause to commit `124a6dec` |
| `node /tmp/test-cargo-toml-updater.mjs` (isolated `CargoToml` test) | `THREW: value at path package.version is not tagged` — confirmed root cause with no network calls |
| `node /tmp/test-root-cargo-toml.mjs` | `THREW: is not a package manifest (might be a cargo workspace)` — confirmed the second failure mode |
| `npx release-please manifest-pr --dry-run --local --local-path=.` (in a scratch clone) | Silently discarded the test config edit before running — led to discovering the `--local` reset behavior |
| `npx release-please manifest-pr --dry-run --target-branch=test/release-please-simple-type` (real pushed branch, no `--local`) | Correctly showed only `CHANGELOG.md`/`version.txt`/manifest as update targets — confirmed the real fix |
| `cargo fmt --all -- --check` | Reproduced the `Format` CI job failure locally before fixing |
| `gh api repos/jmagar/labby/actions/runners` | Confirmed `steamy-lab` offline, then online after the runner fix |
| `ssh steamy-wsl "... Get-Content '...\_diag\Runner_20260706-172139-utc.log' -Tail 40"` | Found the exact abort reason for the dead runner |
| `labby update --version v1.0.0 --dry-run` then without `--dry-run` | Installed `labby 1.0.0` locally and synced it into the Incus container in one command |
| `gh release upload v1.0.0 ...` | Manually attached the already-built artifacts after `release.yml`'s `Create Release` job failed |

## Errors Encountered

- **`release-please` "value at path package.version is not tagged"** (recurred twice).
  Root cause: `release-type: "rust"`'s `CargoToml` updater is fundamentally incompatible
  with this workspace's virtual-root + `version.workspace = true` structure. Resolved by
  switching to `release-type: "simple"` plus a dedicated sync job (see Sequence of
  Events #15–18).
- **Redaction-regex mistake exposed the user's `PYPI_TOKEN` in cleartext** in the
  conversation transcript. Caught immediately, disclosed to the user, and the user was
  advised to rotate the token. No further leaks in subsequent commands (switched to
  key-name-only greps and pipe-direct-from-env patterns for all secret handling
  afterward).
- **`--local --local-path=.` (pointed at the real repo checkout) discarded an uncommitted
  edit.** Non-destructive (the edit was trivial and easily redone), but changed the
  investigation approach going forward — all subsequent config testing used real pushed
  branches or fully isolated scratch clones.
- **`computer-use` access denied by the user**, then **`steamy-windows-mcp` returned
  `RPC server is unavailable`** — neither was a real approval-gate as initially assumed;
  both were dead ends. Resolved by switching entirely to SSH + PowerShell interop.
- **`gh release edit --generate-notes`** in `release.yml` — invalid flag combination,
  caused the `Create Release` job to fail after all build jobs succeeded. Fixed for
  future releases; worked around for `v1.0.0` by uploading assets manually.
- **`Monitor` script crashes** — one used the zsh-reserved variable name `status`
  (`read-only variable: status`), one had a bare `gh`/`jq` call with no `|| true`
  fallback. Both diagnosed from the monitor's own output file and restarted successfully.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `release-please` on `lab` | Failed on every push with `"value at path package.version is not tagged"` | Successfully opens/merges release PRs; `Cargo.toml`/`Cargo.lock` stay in sync via the new `sync-cargo-version` job |
| `OpenWiki Update` workflow (fleet-wide) | Failing on z.ai's 5-hour rate limit | Uses the local `openai-compatible` proxy (`cli-api.tootie.tv`) |
| `steamy-lab` GitHub Actions runner | Dead since 2026-07-06, would only restart on machine reboot, opened a visible console window | Runs hidden with auto-restart on any exit, started immediately without a reboot |
| `release.yml` "Create Release" job | Failed whenever the release already existed (i.e. always, now that `release-please` pre-creates it) | Uploads build artifacts to the existing release without attempting an invalid `edit --generate-notes` |
| `labby` binary (local + Incus container) | `0.30.0` | `1.0.0`, verified matching on both |
| `~/docs` | No dedicated OpenWiki/beads/lavra/xtask/MCP-registry/no-mcp docs existed | 12 new docs + combined `.gitignore` live and chezmoi-tracked |
| `.rustarr` / `.lab` (dotfiles) | chezmoi-managed | Unmanaged (live directories untouched, safe to delete later) |
| `.yarr` / `.labby` (dotfiles) | Unmanaged | Config/`.env` files tracked, encrypted |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `gh pr diff 198` (Cargo.toml/Cargo.lock/manifest hunks) | Only `[workspace.package].version` changed in root `Cargo.toml`; no individual crate `Cargo.toml` touched; 11 matching `Cargo.lock` bumps | Confirmed exactly that (12 `version = "1.0.0"` matches in `Cargo.lock`, zero `crates/*/Cargo.toml` in the diff) | pass |
| `gh run view <ci-run> --json jobs` (post-merge) | All jobs pass | `success` on every job including `ci-gate` | pass |
| `gh release view v1.0.0` | Tag + release exist after `release-please` re-run on the merge commit | `publishedAt: 2026-07-09T03:02:32Z`, `tagName: v1.0.0` | pass |
| `gh release view v1.0.0 --json assets` | All 6 build artifacts attached after manual upload | All 6 listed | pass |
| `labby --version` (local) | `1.0.0` | `labby 1.0.0` | pass |
| `labby update` output | Reports Incus container synced to the same version | `synced Incus container: labby` / `container version: labby 1.0.0` | pass |
| `gh api repos/jmagar/labby/actions/runners` (post-fix) | `steamy-lab` status `online` | `{"status":"online","busy":true}` (immediately picked up the queued CI run) | pass |
| `chezmoi managed ~/docs/.env` | Prints nothing (file must stay untracked) | Confirmed untracked, both before and after adding `BEADS_DOLT_PASSWORD` | pass |

## Risks and Rollback

- The `sync-cargo-version` job pushes a commit directly onto the `release-please`-owned
  PR branch using `RELEASE_PLEASE_TOKEN`. If that token's scope ever narrows, this job
  would fail silently-ish (a red job, not a red merge) — worth a periodic check that it's
  still running successfully on real release PRs.
- The regex-based `Cargo.toml` patch in `sync-cargo-version` matches
  `\[workspace\.package\][^\[]*?\nversion = "..."` — validated once against the current
  file layout. If `[workspace.package]`'s key order ever changes such that `version` is
  no longer the first key after the header, the regex would still work (it's
  order-independent within that block via `[^\[]*?`), but if a second `[workspace.*]`
  sub-table were inserted *before* `version` within the same bracket depth, verify the
  regex still targets the right occurrence.
- Rollback for the release-please config change: revert commit `8a3f5809` (restores
  `release-type: "rust"` and removes the sync job) — but this reintroduces the original
  crash, so only appropriate if reverting alongside a structural change to the Cargo
  workspace (e.g. moving off `version.workspace = true`).

## Decisions Not Taken

- **Manual per-release Cargo.toml bump** (no automation) — rejected in favor of the
  `sync-cargo-version` job to preserve full release automation.
- **Restructure the workspace to give every crate an explicit `version = "x.y.z"`**
  (rather than `version.workspace = true`) — would have made `release-type: "rust"` work
  natively, but is a much larger, invasive change across 11 crates with no clear benefit
  over the chosen fix.
- **Force-applying the `.rustarr` chezmoi permission-mode diff** during dotfiles
  reconciliation — declined because it would have loosened permissions on a
  secrets-adjacent file without understanding why the live value was more restrictive
  (moot now that `.rustarr` has since been retired).
- **Wiring up `memos`'s OpenWiki workflow** — declined; its git remote points at the
  upstream `usememos/memos` project, not the user's own fork, so pushing there would have
  affected a third-party public repository.

## References

- [PR #198 — chore(main): release 1.0.0](https://github.com/jmagar/labby/pull/198)
- [PR #390 — axon OpenWiki proxy switch](https://github.com/jmagar/axon/pull/390)
- [v1.0.0 GitHub Release](https://github.com/jmagar/labby/releases/tag/v1.0.0)
- `release-please` library source: `updaters/rust/cargo-toml.js`,
  `util/toml-edit.ts`, `factories/plugin-factory.js`, `strategies/simple.js`
  (installed at `~/.npm/_npx/dd04923b7fe85367/node_modules/release-please/`)
- `~/docs/dev/megatask-2026-07-08-openwiki-docs-rollout.md` — tracker for the
  fleet-wide documentation rollout referenced throughout this session

## Open Questions

- `.github/CLAUDE.md`'s "Release Process" section still describes a `cargo-release`-based
  flow that no longer matches reality (the repo uses `release-please`). Not fixed this
  session — flagged as a follow-up.
- Whether `sync-cargo-version`'s regex-based `Cargo.toml` patch should be hardened
  (e.g. a proper TOML parse/write instead of regex) if the file structure ever changes
  significantly.
- The four `.claude/worktrees/*` agent worktrees found during this session
  (`elated-khorana-881639`, `great-wiles-864b4c`, `unruffled-keller-bce807`,
  `vigilant-solomon-0ffa4b`) were not investigated — unknown whether they're stale or
  represent other in-progress sessions.

## Next Steps

- Watch the *next* real `release-please` cycle (triggered by ordinary future commits) to
  confirm the `simple` + `sync-cargo-version` combination holds up outside this session's
  hand-validated case.
- Consider updating `.github/CLAUDE.md`'s stale "Release Process" section to describe the
  actual `release-please`-based flow (see Open Questions).
- No immediate action required on the fleet OpenWiki/docs rollout or the `steamy` runner
  fix — both verified working at session end.
