---
title: "Doctor Service"
created: "2026-08-18"
updated: "2026-08-23"
---

# Doctor Service

The `doctor` service is Labby's always-on diagnostic surface for local system, authentication, access-store, OAuth relay, and reverse-proxy readiness. It is available through CLI, MCP, and HTTP API.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact parameters, scopes, and result schemas.

## Current Actions

- `system.checks` runs the local system diagnostic set.
- `auth.check` validates current authentication readiness.
- `access.check` inspects access-store readiness and filesystem safety without creating, migrating, repairing, or otherwise modifying the store.
- `oauth.relay.check` checks callback-relay state and optionally probes configured targets; it requires `lab:admin`.
- `proxy.check` validates a requested app/MCP/backend route combination.
- `proxy.preflight` checks configured reverse-proxy prerequisites.
- `audit.full` streams the combined diagnostic findings, including exactly one local access-store finding.
- `help` and `schema` provide discovery metadata.

Doctor is diagnostic. It reports structured findings and recovery guidance rather than silently repairing state. Repair belongs to the `setup` service.

Local subprocess probes use one process-wide five-probe admission budget across
all concurrent audits, plus per-probe and aggregate deadlines. Dropping an HTTP
SSE audit stream cancels its producer and active subprocess tree; disconnected
clients do not leave detached diagnostic work running.

`system.checks` includes `config:backup-retention`. A warning means the bounded
post-commit retention pass could not converge (more than 10 copies or more than
64 MiB remain). Preserve the newest `config.toml.bak.*` recovery point, verify
the active configuration, and follow the recovery procedure in
[Configuration](../runtime/CONFIG.md); doctor never deletes backups.

## Access-store health

`access.check` returns one agent-safe `access` / `store` finding. It does not expose database paths, SQL, identities, or raw storage errors. Its stable health classifications project to findings as follows:

| Classification | Severity | Operator response |
| --- | --- | --- |
| `ready` | OK | No action required. |
| `missing` | warning | Run the explicit owner-bootstrap workflow before enabling access enforcement. |
| `uninitialized` | warning | Initialize or migrate the store and complete owner bootstrap before enforcement. |
| `insecure` | failure | Secure the state directory, database, and any SQLite sidecars. |
| `corrupt` | failure | Restore or explicitly repair the store before enforcement. |
| `newer_schema` | failure | Upgrade Labby to a version that supports the store schema. |
| `locked` | failure | Retry after the concurrent database operation or checkpoint completes. |
| `read_only` | failure | Restore writable, owner-only access. |
| `unavailable` | failure | Verify the configured path and filesystem availability. |

Missing and uninitialized stores are advisory only while access enforcement remains disabled. All other non-ready states fail closed for readiness. Neither `access.check` nor `audit.full` performs owner bootstrap or changes access-control state.

## Related Docs

- [Setup](./SETUP.md)
- [Operations](../OPERATIONS.md)
- [OAuth](../runtime/OAUTH.md)
- [Reverse proxy](../runtime/REVERSE_PROXY.md)
- [Errors](../dev/ERRORS.md)
