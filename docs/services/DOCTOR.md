---
title: "Doctor Service"
created: "2026-08-18"
updated: "2026-08-18"
---

# Doctor Service

The `doctor` service is Labby's always-on diagnostic surface for local system, authentication, OAuth relay, and reverse-proxy readiness. It is available through CLI, MCP, and HTTP API.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact parameters, scopes, and result schemas.

## Current Actions

- `system.checks` runs the local system diagnostic set.
- `auth.check` validates current authentication readiness.
- `oauth.relay.check` checks callback-relay state and optionally probes configured targets; it requires `lab:admin`.
- `proxy.check` validates a requested app/MCP/backend route combination.
- `proxy.preflight` checks configured reverse-proxy prerequisites.
- `audit.full` streams the combined diagnostic findings.
- `help` and `schema` provide discovery metadata.

Doctor is diagnostic. It reports structured findings and recovery guidance rather than silently repairing state. Repair belongs to the `setup` service.

## Related Docs

- [Setup](./SETUP.md)
- [Operations](../OPERATIONS.md)
- [OAuth](../runtime/OAUTH.md)
- [Reverse proxy](../runtime/REVERSE_PROXY.md)
- [Errors](../dev/ERRORS.md)
