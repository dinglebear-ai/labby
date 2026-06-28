# Lavra Recall PATH Fix

## Metadata

- Date: 2026-05-04 07:35:11 EDT
- Repository: `/home/jmagar/workspace/lab`
- Branch: `bd-work/mcp-gateway-review-remediation`
- Head: `60939ce2`
- Related Bead: `lab-qq8y` (`Resolve ACP full-review findings`)
- Prior session note: `docs/sessions/2026-05-04-acp-review-beads-and-remote-dolt.md`

## User Request

The user asked about the earlier warning:

> Lavra recall/dedup is still blocked locally by `SQLite error: no such module: fts5`.

After identifying the cause, the user asked to fix PATH ordering so the Android SDK `sqlite3` is not selected before the Homebrew SQLite build. The user then corrected that the active zsh config is `~/.config/zsh/.zshrc`, not the legacy `~/.zshrc`.

## Findings

- `.lavra/memory/recall.sh` uses `.lavra/memory/knowledge.db` and prefers SQLite FTS5 search before falling back to grep.
- `.lavra/memory/knowledge.db` contains an FTS5 virtual table named `knowledge_fts`.
- The failing `sqlite3` was `/home/jmagar/Android/Sdk/platform-tools/sqlite3`.
- That Android SDK SQLite binary did not report `ENABLE_FTS5` in `pragma compile_options`.
- A working SQLite exists at `/home/linuxbrew/.linuxbrew/bin/sqlite3` and reports `ENABLE_FTS5`.
- `~/.zshenv` sets `ZDOTDIR="$HOME/.config/zsh"`, so the active interactive zsh config is `~/.config/zsh/.zshrc`.

## Changes Made

- Updated `/home/jmagar/.config/zsh/.zshrc` so `/home/linuxbrew/.linuxbrew/bin` and `/home/linuxbrew/.linuxbrew/sbin` appear before Android SDK paths in the deduped `path=(...)` array.
- Updated `/home/jmagar/.config/.profile` so login and non-interactive shells sourced through `~/.zshenv` also prefer Homebrew before Android SDK paths.
- Reverted the mistaken persistent change to legacy `/home/jmagar/.zshrc` after the user pointed out it is not the active zsh config location.

## Verification

Fresh zsh verification:

```text
sqlite3 -> /home/linuxbrew/.linuxbrew/bin/sqlite3
compile option -> ENABLE_FTS5
```

Lavra recall verification:

```text
.lavra/memory/recall.sh "lab beads dolt" --all
exit code: 0
stderr lines: 0
output lines: 545
```

Android tooling remained available after the PATH change:

```text
adb -> /home/jmagar/.local/bin/adb or Android SDK platform-tools through PATH
sdkmanager -> /home/jmagar/.local/bin/sdkmanager or Android SDK cmdline-tools through PATH
```

Beads/Dolt verification:

```text
bd version 1.0.3 (1b2dd2cb)
Database: lab
Host: 100.64.0.20
Port: 3311
Server connection OK
```

## Follow-Up

- Add Lavra structured comments to `lab-qq8y` for the PATH/FTS5 incident and the active zsh config correction.
- If this pattern recurs elsewhere, Lavra recall could be hardened to select an FTS5-capable SQLite binary or suppress FTS errors before falling back.
