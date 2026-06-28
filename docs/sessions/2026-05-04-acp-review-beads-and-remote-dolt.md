---
date: 2026-05-04 07:18:15 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 60939ce2
agent: Codex
session id: 019ded97-3279-78f0-8ff3-e7cbcd77af60
transcript: /home/jmagar/.codex/sessions/2026/05/03/rollout-2026-05-03T07-26-42-019ded97-3279-78f0-8ff3-e7cbcd77af60.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  60939ce2 [bd-work/mcp-gateway-review-remediation]
pr: #40 Integrate service wave and CI updates https://github.com/jmagar/lab/pull/40
---

# ACP Review, Beads Tracking, and Remote Dolt Repair

## User Request

The session began with a request to run `comprehensive:full-review` scoped to ACP-related code. Follow-up requests were to continue the review, create Beads for everything in the full report, correct Beads/Dolt remote usage, update local Beads tooling, save the session to markdown, and run `lavra-learn`.

## Session Overview

- Completed an ACP-only comprehensive full review and saved phased artifacts under `.full-review/`.
- Created a parent Bead plus child Beads for all distinct actionable findings from the ACP report.
- Corrected a mistaken local-Dolt detour by importing the Beads into the authoritative remote Dolt repo on `node-b`.
- Updated the active local `bd` installation from `0.62.0` to `1.0.3`.
- Fixed the remote Dolt server state so plain `bd` commands against database `lab` work again.

## Sequence of Events

1. Ran the ACP review phases and wrote `.full-review/00-scope.md` through `.full-review/05-final-report.md`.
2. Created Beads for the report; the first attempt accidentally used a repo-local Dolt process after misdiagnosing the remote `database not found: lab` error.
3. User clarified the remote Dolt server was intentional and should be used.
4. Verified `~/.codex/config.toml` and the live shell already had `BEADS_DOLT_*` variables for `100.64.0.20:3311`.
5. Diagnosed the real failure: local `bd` was old and the remote Dolt SQL server had `lab` visible but plain `lab` sessions failed until server state was repaired.
6. Imported the ACP Beads into `/mnt/appdata/dolt/lab` on remote host `node-b` and removed the accidental local-only records.
7. Updated active `bd` on PATH to `1.0.3`, restored remote metadata/port settings, stopped the accidental local Lab Dolt process, and restarted the remote Dolt container.

## Key Findings

- `~/.codex/config.toml:330` through `~/.codex/config.toml:334` correctly set `BEADS_DOLT_SERVER_HOST`, `BEADS_DOLT_SERVER_PORT`, `BEADS_DOLT_SERVER_USER`, `BEADS_DOLT_SERVER_TLS`, and `BEADS_DOLT_PASSWORD`.
- The active shell inherited those variables; the initial remote failure was not caused by stale shell configuration.
- The active `bd` on PATH was an old fnm/npm shim at version `0.62.0`; `~/.local/bin/bd` and the remote host binary were already `1.0.3`.
- The authoritative remote Dolt data for this repo is on `node-b` at `/mnt/appdata/dolt/lab`.
- A bad persisted Dolt config key, `sqlserver.global.lab_default_branch`, was created during diagnosis and then removed from the remote container.

## Technical Decisions

- Used `.full-review/05-final-report.md` as the source of truth for Bead creation.
- Created one parent epic, `lab-qq8y`, and 16 child Beads instead of duplicating test/doc findings as separate tickets when they belonged with implementation tickets.
- Preserved the user's unrelated dirty worktree and did not stage or revert source changes.
- Used direct remote Dolt access with `sudo -n dolt --data-dir /mnt/appdata/dolt/lab` only after normal `bd` was blocked, then repaired normal `bd` usage.
- Restarted only the remote Dolt Docker container after removing the bad persisted config so `@@lab_head_ref` rebuilt as `refs/heads/main`.

## Files Modified

- `.full-review/00-scope.md` through `.full-review/05-final-report.md` — ACP review artifacts.
- `.beads/metadata.json` — restored to remote Dolt endpoint `100.64.0.20:3311`.
- `.beads/dolt-server.port` — restored to `3311`.
- `docs/sessions/2026-05-04-acp-review-beads-and-remote-dolt.md` — this session note.
- Active global npm package `@beads/bd` — upgraded from `0.62.0` to `1.0.3`.

## Commands Executed

- `bd --version`, `bd dolt show`, `bd list --json --id lab-qq8y -n 0` — verified Beads CLI version, remote endpoint, and epic visibility.
- `npm install -g @beads/bd@1.0.3` — updated the active `bd` package used by the shell.
- `ssh 100.64.0.20 'sudo -n dolt --data-dir /mnt/appdata/dolt/lab ...'` — verified and imported Beads into the authoritative remote Dolt repo.
- `ssh 100.64.0.20 'docker restart 08b4bf3de7ac'` — restarted the remote Dolt container after removing bad persisted config.
- `kill $(cat .beads/dolt-server.pid)` — stopped the accidental repo-local Lab Dolt process.
- `chmod 700 .beads` — fixed Beads permission warning.

## Errors Encountered

- `bd list` initially failed with `database not found: lab` against the remote server. The live env was correct; the real issue was remote Dolt session/database behavior plus an old local `bd` binary.
- I mistakenly overrode commands to a local Dolt server at `127.0.0.1:45539`. That created local-only Beads, which were later deleted from the local store and committed as a cleanup.
- Direct SQL writes to remote `lab/main` over the SQL server failed with `Unknown system variable '_head_ref'`. Direct repo access on the remote host via `/mnt/appdata/dolt/lab` succeeded.
- `.lavra/memory/recall.sh "beads dolt acp" --all` failed with `no such module: fts5`; this affects recall/dedup lookup for `lavra-learn` unless the SQLite FTS module issue is fixed.

## Behavior Changes

- Before: active `bd` was `0.62.0`, normal `bd list` failed against the remote `lab` database, and repo-local Lab Dolt metadata/port had been temporarily pointed at the local process.
- After: active `bd` is `1.0.3`, normal `bd list --id lab-qq8y` reads the remote epic, remote metadata points to `100.64.0.20:3311`, and the accidental local Lab Dolt process is stopped.

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `bd --version` | `1.0.3` | `bd version 1.0.3 (1b2dd2cb)` | pass |
| `bd dolt show` | remote `100.64.0.20:3311`, database `lab` | remote host/port and database shown; connection OK | pass |
| `bd list --json --id lab-qq8y -n 0` | parent epic exists | returned `lab-qq8y Resolve ACP full-review findings open` | pass |
| `bd list --json --parent lab-qq8y -n 0` | 16 children | returned `16` | pass |
| `ss -ltnp \| rg ':45539\|:42211\|dolt'` | no Lab repo-local Dolt listener | only `axon_rust` Dolt process remained on `127.0.0.1:42211` | pass |
| remote Dolt log | ACP import commit exists | `enuvsge8165q52umrh4tp63fi5gnk1o4 Import ACP full-review beads lab-qq8y` | pass |

## Risks and Rollback

- The current git worktree is heavily dirty with unrelated changes. This session intentionally avoided reverting or staging them.
- `docs/sessions/` is ignored by `.gitignore`, so this note will not be included in a normal `git add .` unless force-added.
- Remote Dolt container restart was targeted to the Beads SQL container `08b4bf3de7ac`; rollback would be another container restart if needed.
- The accidental local Beads records were removed from the local Lab Dolt store; authoritative records remain on the remote Dolt repo.

## Decisions Not Taken

- Did not modify `~/.codex/config.toml`; it was already correctly setting the remote Beads environment variables.
- Did not replace `~/.local/bin/bd`; it was already `1.0.3`.
- Did not stage or commit repo source changes.
- Did not create separate Beads for every repeated test/doc mention when a single implementation Bead could own the whole remediation thread.

## References

- `.full-review/05-final-report.md`
- `docs/sessions/2026-05-04-acp-review-beads-and-remote-dolt.md`
- `~/.codex/config.toml`
- Remote Dolt repo: `node-b:/mnt/appdata/dolt/lab`
- Remote import commit: `enuvsge8165q52umrh4tp63fi5gnk1o4`
- PR: `https://github.com/jmagar/lab/pull/40`

## Open Questions

- Why `bd`/Dolt created or tolerated the bad persisted `sqlserver.global.lab_default_branch` entry, and whether this should be guarded in Beads tooling.
- Why `.lavra/memory/recall.sh` fails with missing SQLite `fts5` in this environment.

## Next Steps

- Started but not completed: run `lavra-learn` after this save note and record any knowledge comments added.
- Follow-on: fix or document the Lavra `fts5` recall dependency if knowledge deduplication remains blocked.
- Follow-on: begin remediation on `lab-qq8y.1` through `lab-qq8y.16` when ready.
