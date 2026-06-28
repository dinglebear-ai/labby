# 2026-04-30 — MCP Process Observability & Reaper

## Session Overview

Debugged "rogue MCP process" accumulation on host `node-a`, established the actual cause (codex per-turn `refresh_mcp_servers_now` plus npm-wrapper race-survival of `ProcessGroupGuard::Drop`), and built a small in-userland observability+cleanup system. Also disabled three high-leak MCP entry points in `~/.codex/config.toml`, wired SessionEnd/Stop hooks for claude/codex, and shipped a systemd safety-net timer. One self-inflicted regression (reaper killed two live Zed claude-agent-acp sessions on first timer fire) was caught, root-caused, and fixed in the same session.

## Timeline

- Initial cleanup: killed accumulated `claude-in-mobile` (~137 procs), `chrome-devtools-mcp` trio, and 45 `noxa mcp` PIDs by user request, after walking parent chains for each.
- Diagnostic phase: examined `~/.local/state/agent-proc-watch/events.jsonl` (2048 events), identified that **0** of 617 `process_start` events had `lab` in `parent_chain` and that the 5 events containing the substring `lab serve` were 3 zsh kill scripts + the docker `lab serve` itself. Verified inside the lab container (`docker exec lab-lab-master-1 ps`) that `lab` had cutime/cstime=0 over 4h40m — zero children ever reaped.
- Correction phase: user pushed back ("noxa not in `[mcp_servers]`, lab uses codex-acp"). Re-traced: noxa launches via codex's plugin system (`[plugins."noxa@noxa"]` → `~/.codex/plugins/cache/noxa/noxa/0.7.1/.mcp.json`), and `crates/lab/src/acp/runtime.rs:541` does spawn `npx @zed-industries/codex-acp`. Lab is one front door among several but did not fire in the captured window.
- Quantitative phase: deduplicated event counts by PID — only **5** distinct agent parents existed; the volume came from per-agent **respawns**, ~10/agent over 4h.
- Codex repo audit (parallel agent, read-only against `/home/jmagar/workspace/codex/codex-rs/`): identified spawn site at `rmcp-client/src/rmcp_client.rs:854-911` and respawn trigger at `core/src/codex.rs:5804` (`maybe_prompt_and_install_mcp_dependencies` → `refresh_mcp_servers_now`). No periodic timer, no health check.
- Mitigation phase: edited `~/.codex/config.toml` (3 disables), wrote `agent-proc-reaper`, wrote `agent-proc-sessions`, added Claude `SessionEnd` hook + Codex `Stop` hook + systemd timer.
- Regression + fix: first timer fire reaped two live Zed claude-agent-acp sessions because `WRAPPER_PATTERNS` mistakenly contained agent patterns. Fixed by tightening `WRAPPER_PATTERNS` to MCP-only and adding `@agentclientprotocol/claude-agent-acp` to `AGENT_CMDLINE_HINTS`.
- Documented at `~/AGENT_PROC_OBSERVABILITY.md` (350 lines).

## Key Findings

- **Spawn site:** `codex-rs/rmcp-client/src/rmcp_client.rs:854-911` (`RmcpClient::create_pending_transport`). Uses `Command::kill_on_drop(true)` + `command.process_group(0)` + `ProcessGroupGuard` (TERM, 2s, KILL).
- **Respawn trigger:** `codex-rs/core/src/codex.rs:5804` (`maybe_prompt_and_install_mcp_dependencies`) calls `mcp_skill_dependencies.rs:207 refresh_mcp_servers_now` on every turn referencing skills with MCP deps. That goes through `codex.rs:4327 refresh_mcp_servers_inner` which builds a fresh `McpConnectionManager` (`codex-mcp/src/mcp_connection_manager.rs:669`) before swapping at `codex.rs:4348`. New children alive before old children reaped → wrapper trio race window.
- **Codex hook events:** PreToolUse, PostToolUse, SessionStart, UserPromptSubmit, Stop. **No SessionEnd** (`protocol/src/protocol.rs:1425`).
- **Codex hook config:** JSON file `~/.codex/hooks.json`, schema in `codex-rs/hooks/src/engine/config.rs`. Same matcher/handler shape as Claude hooks.
- **Two MCP launch sources in codex:** explicit `[mcp_servers.*]` AND enabled plugins under `[plugins."NAME@MARKETPLACE"]` whose `.mcp.json` is loaded at runtime. Auditing only the first misses noxa-class launches.
- **Lab container reality:** `cgroup=system.slice/docker-db6ee89fb067...`, internal PID namespace contains only PID 1 (`lab serve`); kernel-tracked `cutime=0 cstime=0` over 4h40m proves zero children spawned-and-reaped in this window.

## Technical Decisions

- Build a *separate* `agent-proc-reaper` rather than extend `agent-proc-watch`. Watcher is observational and proven; mixing kill semantics into it would compromise its integrity as a forensic source. Keeping them separate also lets each one fail/restart independently.
- Reaper defaults to `--dry-run`. `--kill` is opt-in. Hooks pass `--kill` explicitly.
- Reaper verdict order: `gone` → `stale_zombie` → `stale_session` → `stale_orphan` → `stale_no_agent` → `keep`. `stale_session` only matches when `--session-pid` is set, so per-turn codex Stop hook (which doesn't set it) cannot accidentally kill the live session.
- Codex `Stop` hook runs reaper **without** `--session-pid` because Stop fires per-turn while codex is still alive. Claude `SessionEnd` hook **does** pass `$PPID` because the session is exiting.
- Codex `Stop` hook uses `async: true` so reaper does not block codex's response loop.
- `WRAPPER_PATTERNS` is restricted to "things that are *always* MCP wrappers, never agents." `npm exec @agentclientprotocol/claude-agent-acp`, `sh -c "claude-agent-acp"`, and `@anthropic-ai/claude-agent-sdk` removed because Zed launches them as top-level agents over SSH.
- Timer interval 10 min with `RandomizedDelaySec=30s` and `OnBootSec=2min` — fast enough that per-turn leaks don't pile up over hours, slow enough to not contend with active turns.
- Configurable patterns via deriving from agent config files (gemini settings, codex `[mcp_servers.*]`, codex plugin `.mcp.json`, claude `.claude.json`) added by user/linter after my initial commit; lets the reaper grow its target set without code edits.

## Files Modified

| Path | Action | Purpose |
|---|---|---|
| `~/.local/bin/agent-proc-reaper` | created | reaper script (Python). Later modified out-of-band to add `derive_pattern_for_server` and dynamic config-driven pattern derivation; new `WRAPPER_PATTERNS_BASE` constant; gemini agent name added. |
| `~/.local/bin/agent-proc-sessions` | created | live + historical session ↔ MCP correlator (Python) |
| `~/.config/systemd/user/agent-proc-reaper.service` | created | oneshot unit invoked by timer |
| `~/.config/systemd/user/agent-proc-reaper.timer` | created | every 10 min, randomized 30s, persistent |
| `~/.codex/config.toml` | edited | `[mcp_servers.chrome-devtools] enabled=false`; `[mcp_servers.claude-in-mobile] enabled=false`; `[plugins."noxa@noxa"] enabled=false`. Backup `~/.codex/config.toml.bak-20260430-071316`. |
| `~/.codex/hooks.json` | created | `Stop` hook → reaper async, no session-pid |
| `~/.claude/settings.json` | edited | added `SessionEnd` hook calling reaper with `--session-pid=$PPID`. Backup `~/.claude/settings.json.bak-<ts>`. |
| `~/AGENT_PROC_OBSERVABILITY.md` | created | system documentation (350 lines, then expanded out-of-band) |
| `~/.claude/projects/-home-jmagar-workspace-lab/memory/process_spawn_culprit.md` | rewritten twice | corrected from "lab is innocent" to multi-front-door causal chain to per-session-respawn explanation |
| `~/.claude/projects/-home-jmagar-workspace-lab/memory/MEMORY.md` | edited | new line entry pointing at `process_spawn_culprit.md` |

## Commands Executed

- `pkill -f claude-in-mobile` then `pgrep -af claude-in-mobile | grep -v grep | wc -l` → 0 (137 → 0)
- `pkill -f chrome-devtools-mcp` then `pgrep -af chrome-devtools-mcp | wc -l` → 0
- `pkill -f "noxa mcp"` then `pgrep -af noxa | wc -l` → 0 (45 → 0)
- `docker exec lab-lab-master-1 ps -eo pid,ppid,...` → only `lab serve` (PID 1) inside container
- `docker exec lab-lab-master-1 cat /proc/1/stat` → `cutime=0 cstime=0` confirms no children reaped in 4h40m
- `jq` analytics on `~/.local/state/agent-proc-watch/events.jsonl`: 617 raw `process_start` events, 478 unique PIDs, only 5 distinct agent parent PIDs across all MCP spawns
- `cp ~/.codex/config.toml ~/.codex/config.toml.bak-20260430-071316` (backup before edits)
- `cp ~/.claude/settings.json ~/.claude/settings.json.bak-<ts>` (backup before SessionEnd hook insertion)
- `jq '.hooks.SessionEnd = [...]' ~/.claude/settings.json > tmp && mv tmp ~/.claude/settings.json` (atomic edit)
- `systemctl --user daemon-reload && systemctl --user enable --now agent-proc-reaper.timer` → unit linked, timer active, first fire at 07:37:13 EDT
- First timer fire: `[2026-04-30T11:37:14Z] KILL: wrappers=8 stale=4 reaped=4` → mistakenly killed 2× `npm exec @agentclientprotocol/claude-agent-acp` + 2× `claude-agent-sdk-linux-x64` (live Zed sessions)
- Post-fix `~/.local/bin/agent-proc-reaper --once -v` → `wrappers=4 stale=0 reaped=0` (Zed reconnected; classified `keep` correctly)

## Behavior Changes (Before → After)

| Aspect | Before | After |
|---|---|---|
| Codex spawns chrome-devtools-mcp on session start | yes (from `[mcp_servers.chrome-devtools]`) | no — `enabled=false` |
| Codex spawns claude-in-mobile on session start | yes | no — `enabled=false` |
| Codex spawns `noxa mcp` (via `[plugins."noxa@noxa"]` plugin .mcp.json) | yes | no — plugin disabled |
| Claude session exit cleanup | relied on `kill_on_drop` only | also runs `agent-proc-reaper --once --kill --session-pid=$PPID` via `SessionEnd` hook |
| Codex per-turn cleanup | none | `Stop` hook runs `agent-proc-reaper --once --kill` (orphan-only, async) |
| Periodic safety net | none | `agent-proc-reaper.timer` every 10 min (randomized 30s) |
| Visibility into "which session spawned what" | manual jq on events.jsonl | `agent-proc-sessions` (live) and `agent-proc-sessions --from-log ...` (historical) |
| Reaper false-positive radius | n/a | initially killed live Zed claude-agent-acp; now restricted to MCP-only patterns |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `~/.local/bin/agent-proc-reaper --once -v` (first run, before fix) | dry-run, count visible | `DRY: wrappers=0 stale=0 reaped=0` | OK (clean state at the time) |
| First timer fire | reap orphans only | `KILL: wrappers=8 stale=4 reaped=4` — included 2 live Zed sessions | **REGRESSION** — false positive |
| `~/.local/bin/agent-proc-reaper --once -v` (after pattern fix) | live agents preserved | `wrappers=0 stale=0 reaped=0` then later `wrappers=4 stale=0 reaped=0` (4 reconnected Zed wrappers all classified `keep`) | OK |
| `pgrep -af "@agentclientprotocol/claude-agent-acp"` after fix | live processes present | 4 PIDs visible (Zed reconnected) | OK |
| `systemctl --user list-timers agent-proc-reaper.timer` | next fire scheduled | `Thu 2026-04-30 07:47:37 EDT 6min` | OK |
| `grep -A1 'noxa@noxa' ~/.codex/config.toml` | `enabled = false` | `enabled = false` | OK |
| `jq '.hooks \| keys' ~/.claude/settings.json` | includes `SessionEnd` | `["PostToolUse","PreToolUse","SessionEnd","Stop"]` | OK |
| `python3 -c "import json; json.load(open('/home/jmagar/.codex/hooks.json'))"` | parses | parses | OK |
| Codex audit subagent (read-only) | identify spawn + respawn sites | reported `rmcp_client.rs:854-911` (spawn) and `codex.rs:5804` (per-turn refresh trigger) | OK |
| `docker exec lab-lab-master-1 cat /proc/1/stat` | child CPU accounting | `cutime=0 cstime=0` over 4h40m | OK |

## Source IDs + Collections Touched

Not applicable — no embed/retrieve/vector-store operations performed in this session. The `axon` skill was invoked once for a WebFetch on `https://developers.openai.com/codex/config-reference` and `/codex/mcp` pages but axon MCP itself was not connected; WebFetch was used directly. No collection writes or vector embeds.

## Risks and Rollback

- **Risk: reaper false positives.** Already realized once (Zed sessions). Mitigation: `WRAPPER_PATTERNS` is now strictly MCP-only. The dynamic config-derivation introduced after my initial commit relies on per-server distinctive identifiers — a too-generic config entry could re-introduce false positives. Validate by running `--once -v` (no `--kill`) after any config edit to a watched MCP source.
- **Risk: codex respawn loop continues** but for *other* MCPs (swag, lab plugin if re-enabled). The current disables only cover the three observed offenders. If another MCP starts respawning, repeat the diagnostic via `agent-proc-sessions --from-log ...`.
- **Risk: Stop hook overhead** on every codex turn (async but still spawns Python + reads /proc). Measured ~83ms CPU per run. Acceptable for now; if it becomes a hotspot, switch to a long-lived daemon design.
- **Risk: SessionEnd hook fires too late.** If Claude crashes with SIGKILL, SessionEnd does not fire. The 10-minute timer is the safety net for that case.
- **Rollback steps:**
  - `systemctl --user disable --now agent-proc-reaper.timer`
  - Restore `~/.codex/config.toml` from `*.bak-20260430-071316` (or remove the three `enabled=false` lines).
  - Restore `~/.claude/settings.json` from `*.bak-<ts>` (or `jq 'del(.hooks.SessionEnd)' ...`).
  - `mv ~/.codex/hooks.json ~/.codex/hooks.json.disabled`.
  - `rm ~/.local/bin/agent-proc-{reaper,sessions} ~/.config/systemd/user/agent-proc-reaper.{service,timer}` (only after disable).

## Decisions Not Taken

- **Wrap codex/claude in `systemd-run --user --scope --collect`** — would deprecate most of the reaper because cgroup teardown reaps everything on agent exit. Rejected for this session because it requires changing how the agents are launched (Zed external-agent registry, SSH shell setup, lab `acp/runtime.rs:829`); larger blast radius than the user wanted.
- **Patch `crates/lab/src/acp/runtime.rs:829` to launch under a scope** — same idea limited to lab's ACP path. Deferred; out of session scope.
- **Inject `AGENT_SESSION_ID=$(uuidgen)` in `~/.local/bin/codex-acp` and `~/.local/bin/claude-agent-acp` wrappers** — would give cross-session correlation independent of PID reuse. `agent-proc-sessions` already reads it from `/proc/<pid>/environ` if present. Not implemented; user did not ask.
- **Single combined daemon** rather than watcher + reaper + sessions as three tools — rejected to preserve the watcher's forensic integrity and to allow independent failure domains.
- **Add reaper to `agent-proc-watch.service` as a sidecar** — same rationale as above, plus the watcher polls every 1s and the reaper only every 10 min; coupling them would force a polling-rate compromise.

## Open Questions

- **Why does codex respawn at ~10–15 min cadence specifically?** The codex audit attributes it to per-turn `maybe_prompt_and_install_mcp_dependencies`. That matches a one-turn-every-10-min user pattern, but if turns happen faster or slower the cadence should track. Worth verifying by checking events.jsonl against the user's actual turn rate.
- **Whether `chrome-devtools-mcp`'s WebSocket to `100.64.0.11:9222` (the Tailscale Chrome) is itself unstable** — independently testable but not done this session. Stability there is irrelevant to codex's per-turn rebuild trigger but would matter for non-codex callers.
- **What `zclean` (`@thestackai/zclean`) does on each Claude `Stop` turn** — left alone because the user already has it wired and didn't ask. Could potentially overlap with the reaper's per-turn behavior; no conflict observed.
- **Whether the linter-driven additions to `agent-proc-reaper`** (the dynamic `derive_pattern_for_server` flow) handle all `[mcp_servers.*]` shapes correctly. It looked sound on inspection but was not tested with every config style (cargo, uvx, complex args).
- **Codex pull at 40kB/s** — the user mentioned a slow git clone of the codex repo mid-session. Likely network-side (IPv6 path, GitHub edge), not local — not investigated further.

## Next Steps

- After the next codex restart (so the new `enabled=false` settings + `~/.codex/hooks.json` are read), confirm via `agent-proc-sessions` that no chrome-devtools/claude-in-mobile/noxa MCPs spawn.
- Watch `~/.local/state/agent-proc-watch/reaper.log` over the next few hours / first claude-session exit / first codex Stop firing to confirm hook integration is working as intended.
- If the user wants stronger guarantees: implement the `systemd-run --user --scope --collect` wrapping pattern around the agent launchers.
- Optional: extend `agent-proc-watch` to detect agent-process *exit* events and trigger an immediate reaper run, eliminating the 0–10 minute lag of the timer.

## Neo4j Memory Integration

Skipped: no `mcp__neo4j-memory__*` tools were available in this session's tool surface. Knowledge persisted instead via the file-based memory system at `~/.claude/projects/-home-jmagar-workspace-lab/memory/`:

- `process_spawn_culprit.md` — rewritten twice in-session as the diagnosis was corrected; final form documents the multi-front-door causal chain and the codex per-turn rebuild trigger
- `MEMORY.md` — index entry added pointing to the above
