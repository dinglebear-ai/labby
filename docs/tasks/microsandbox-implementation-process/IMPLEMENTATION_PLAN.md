# Implementation plan

1. Recover and verify prior evidence; preserve host changes and existing staging.
2. Preflight native and MCP runtime paths, inspect sandbox state, and verify authorized SSH publication without changing a remote ref.
3. Create a branch inside the existing persistent development sandbox from current origin/main. Preserve the old validation branch.
4. Write failing receipt tests; implement validation and real-loopback probes; patch review findings; rerun tests and documentation gates.
5. Package the reusable skill, reference bootstrap, fixture, instructions, and task record.
6. Commit only this task, export a bundle, import through an isolated bare broker, publish exactly that SHA, verify the remote ref, and create an unmerged PR.
7. Build a Git archive from that commit. Record its checksum and complete staging configuration outside the commit to avoid circular self-identification.
8. Create a separately named staging VM from the pinned base and locked bootstrap, not a development snapshot. Verify artifact transfer, configuration, logs, HTTP behavior, and source identity externally. Retain staging.

Rollback: leave the previous staging VM intact. Stop only this task’s newly created VM if it is unhealthy. Preserve the branch, bundle, and failed receipts. Never roll back production for this workflow.
