---
date: 2026-04-27 07:20:00 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: 80d23563
plan: none
agent: Codex
working directory: /home/jmagar/workspace/lab
---

## User Request

Create a session note capturing the node deployment/check-in investigation and preserve current diagnosis and open questions.

## Session Overview

I verified the current `/nodes` and `/v1/nodes` behavior against the deployed/enrolled node set. The operator confusion (“only vivobook is checking in”) is explained by the distinction between “enrolled” nodes and currently checking-in/active nodes in the node store.

## Findings

1. `/nodes` in the UI reflects `/v1/nodes` only.
2. `/v1/nodes` currently returns only `vivobook` as connected/active.
3. `lab nodes enrollments list` still shows additional approved device enrollments (`backup-node`, `controller`, `node-b`, `workstation`, `vivobook`).
4. This means deployment and enrollment are not equal to active reporting; some nodes have not completed check-in yet or are offline.

## Commands Executed

```bash
cd /home/jmagar/workspace/lab
target/release/lab nodes list --json
target/release/lab nodes enrollments list
```

## Command Results

`lab nodes list --json`:

```json
[{"node_id":"vivobook","connected":true,"role":"node","log_count":0,"discovered_config_count":0}]
```

`lab nodes enrollments list` (approved subset shown):

```text
▸ approved
  ▸ backup-node
  ▸ node-b
  ▸ workstation
  ▸ controller
  ▸ vivobook
```

## Operational Context

Recent deploy operations were rerun to target the node set and then focused on nodes that were not reporting check-in. `backup-node` remained unreachable during deploy preflight due to SSH timeout on port 22, indicating connectivity/infrastructure issue rather than deployment packaging.

## Root-Cause Summary

- `/nodes` is showing connected nodes only by design.
- Enrolled-but-not-connected nodes do not appear in the cards until they complete registration/check-in after startup.
- A node that was deployed can still be absent in `/v1/nodes` if the process is not running, not restarted, or cannot reach the server endpoint.

## Next Steps

1. Validate the runtime environment on each non-connected approved node (`backup-node`, `node-b`, `workstation`, `controller`) and confirm the node service is running.
2. Check each node for websocket connectivity to the central server (port/protocol/firewall and network ACL checks).
3. Confirm node binary startup order/logs on each target once service is launched.
4. Re-run `lab nodes list --json` after each remediation to verify node moves from enrolled-only to active.

## Open Questions

- Is `backup-node` intentionally firewalled from SSH (needed for deploy and first contact)?
- Are the non-Vivobook nodes expected to report heartbeats on a periodic schedule or only after reboot?
- Should `lab nodes list` include an explicit “enrolled not checked-in” view or row to make this distinction explicit in the UI?
