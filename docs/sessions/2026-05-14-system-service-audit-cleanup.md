---
date: 2026-05-14 23:28:08 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 93392f8a
agent: Codex
session id: 96b07a25-9c53-449b-b4b4-a30205de9a10
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/96b07a25-9c53-449b-b4b4-a30205de9a10.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  93392f8a [main]
---

# System Service Audit And Cleanup

## User Request

The session shifted from process cleanup and MainThread/MCP process inspection into a host-level service cleanup request: locate the running OpenClaw service, disable it, remove its unit file, audit current system services for cruft, and save the audit report in the repo.

## Session Overview

- Located and removed `openclaw-gateway.service`, a user systemd unit running OpenClaw on port `18789`.
- Audited running user services, system services, timers, sockets, Docker containers, open ports, snap services, user sessions, and major memory users.
- Created the repo-local audit report at `docs/system-service-audit-2026-05-15.md`.
- Removed the stale disabled system unit `/etc/systemd/system/lab.service`.
- Clarified that Docker `labby` is the intended Lab runtime, not a systemd service.

## Sequence Of Events

1. Reviewed current agent/MCP process state and explained that `MainThread` entries were Node-based MCP or dev processes, not one service.
2. Located OpenClaw as a user systemd unit: `openclaw-gateway.service`.
3. Inspected the OpenClaw unit definition, active process, memory usage, restart behavior, and listener on port `18789`.
4. Disabled and stopped OpenClaw, removed `/home/jmagar/.config/systemd/user/openclaw-gateway.service`, and reloaded user systemd.
5. Audited running services and saved a markdown report first under `/home/jmagar/docs`, then moved it into this repo at the user's direction.
6. Checked whether `labby` had a systemd unit and confirmed it runs through Docker Compose.
7. Removed stale `/etc/systemd/system/lab.service` with `sudo`, reloaded systemd, and updated the report.
8. Explained `user@1000.service` and `user@60578.service` as normal per-user systemd managers for `jmagar` and `gdm-greeter`.

## Key Findings

- OpenClaw was launched by `/home/jmagar/.config/systemd/user/openclaw-gateway.service` with `Restart=always`, running `node .../openclaw/dist/index.js gateway --port 18789`.
- OpenClaw was consuming about `330.6M` memory at inspection time, with a reported peak of `1.1G`.
- Docker `labby` is the expected runtime:
  - container name: `labby`
  - image: `labby:dev`
  - compose project: `lab`
  - compose service: `labby-master`
  - compose files: `/home/jmagar/workspace/lab/docker-compose.yml` and `/home/jmagar/workspace/lab/docker-compose.dev.yml`
  - restart policy: `unless-stopped`
- The stale system unit `/etc/systemd/system/lab.service` was disabled/inactive and ran `/usr/local/bin/lab serve`; it was not the Docker `labby` runtime.
- `user@1000.service` is the user manager for `jmagar`; `user@60578.service` is the GDM greeter user manager.

## Technical Decisions

- Removed only explicitly requested stale units: OpenClaw and `/etc/systemd/system/lab.service`.
- Treated other findings as audit findings rather than deleting more services without approval.
- Kept `labby` running because the user clarified it is supposed to run via Docker.
- Moved the audit report into the repo because host-level notes for this Lab environment should live with the repo documentation for this request.

## Files Modified

- `docs/system-service-audit-2026-05-15.md`
  - Created service audit report.
  - Updated after `/etc/systemd/system/lab.service` was removed.
  - Clarified that `labby` is the intended Docker runtime.
- `docs/sessions/2026-05-14-system-service-audit-cleanup.md`
  - This session note.

Host files removed:

- `/home/jmagar/.config/systemd/user/openclaw-gateway.service`
- `/etc/systemd/system/lab.service`

## Commands Executed

Critical commands included:

```bash
systemctl --user status openclaw-gateway.service --no-pager
systemctl --user cat openclaw-gateway.service
systemctl --user disable --now openclaw-gateway.service
rm -f /home/jmagar/.config/systemd/user/openclaw-gateway.service
systemctl --user daemon-reload
ss -ltnp | rg '18789|openclaw'
docker inspect labby --format '...'
sudo rm -f /etc/systemd/system/lab.service
sudo systemctl daemon-reload
systemctl status lab.service --no-pager
loginctl list-users --no-legend
loginctl show-user 1000 -p Name -p UID -p Linger -p State -p Sessions -p Timestamp
loginctl show-user 60578 -p Name -p UID -p Linger -p State -p Sessions -p Timestamp
```

## Errors Encountered

- Removing `/etc/systemd/system/lab.service` without elevated permissions failed with `Permission denied`.
  - Resolution: reran the removal via `sudo rm -f /etc/systemd/system/lab.service`, then reloaded systemd.
- `systemctl reset-failed lab.service` reported `Unit lab.service not loaded` after removal.
  - This was expected after the unit file was removed and daemon reload completed.

## Behavior Changes

Before:

- OpenClaw was enabled as a user systemd service and listening on port `18789`.
- A stale disabled system unit existed at `/etc/systemd/system/lab.service`.
- The service audit report was initially outside the repo.

After:

- OpenClaw unit is removed, the OpenClaw process is gone, and port `18789` is no longer listening.
- `/etc/systemd/system/lab.service` is removed and `systemctl` cannot find it.
- The audit report is in the repo at `docs/system-service-audit-2026-05-15.md`.

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `systemctl --user status openclaw-gateway.service --no-pager` | Unit not found after removal | `Unit openclaw-gateway.service could not be found.` | Pass |
| `ss -ltnp \| rg '18789\|openclaw'` | No listener | No output | Pass |
| `test ! -e /home/jmagar/.config/systemd/user/openclaw-gateway.service` | File absent | `unit_file_absent=0` from shell test convention | Pass |
| `docker inspect labby --format ...` | Confirm Docker runtime | `compose_project=lab`, `compose_service=labby-master`, `restart=unless-stopped` | Pass |
| `test ! -e /etc/systemd/system/lab.service` | File absent | `removed` | Pass |
| `systemctl status lab.service --no-pager` | Unit not found | `Unit lab.service could not be found.` | Pass |
| `test -s docs/system-service-audit-2026-05-15.md && wc -l ...` | Report exists | Report existed; after final move, repo path is current | Pass |

## Risks And Rollback

- Removing OpenClaw means whatever depended on `gateway --port 18789` will no longer start automatically.
  - Rollback requires recreating the user unit with the previous `ExecStart` and enabling it with `systemctl --user enable --now openclaw-gateway.service`.
- Removing `/etc/systemd/system/lab.service` means the old host-level `lab serve` unit is gone.
  - Rollback requires recreating that unit or reinstalling whatever created it.
- The audit report is informational; most cleanup candidates were not removed.

## Decisions Not Taken

- Did not stop Docker `labby` after the user clarified it is supposed to run via Docker.
- Did not remove Docker containers, snap packages, remote access services, or cleanup timers without explicit approval.
- Did not remove `user@1000.service` or `user@60578.service`; both are normal systemd user managers.

## References

- `docs/system-service-audit-2026-05-15.md`
- `systemctl --user status openclaw-gateway.service`
- `systemctl status lab.service`
- `docker inspect labby`
- `loginctl show-user 1000`
- `loginctl show-user 60578`

## Open Questions

- Which Docker playground/MCP containers should remain persistent?
- Should `dockersocket` continue exposing Docker socket proxy on `0.0.0.0:2375`?
- Should failed `syslog-ai-index.service` and `syslog-ai-watch.service` be fixed, disabled, or removed?
- Should old disabled system units such as `coredns.service` and `ollama.service` be removed next?

## Next Steps

Unfinished work from this session:

- None of the non-requested cleanup candidates were removed.

Follow-on tasks:

- Decide which report cleanup candidates to remove.
- If continuing cleanup, start with disabled stale units and failed user units before touching remote access or Docker infrastructure.
