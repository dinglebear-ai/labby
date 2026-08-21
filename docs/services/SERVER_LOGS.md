---
title: "Server Logs Service"
updated: "2026-08-18"
---

# Server Logs Service

The `server_logs` service reads and filters Labby's own server-process logs. It does not ingest syslog, collect fleet logs, or act as a general host log aggregator.

The generated [action catalog](../generated/action-catalog.md) is authoritative for the exact action schema.

## Surfaces

`server_logs` is always available through CLI, MCP, and HTTP API. The query action requires `lab:admin`.

## Querying Logs

`server_logs.query` supports bounded filtering by fields including log level, tracing target, service, action, error kind, free-text query, file, result limit, and maximum scan bytes.

Callers should treat the returned structure as the stable product response and should not parse raw journal output themselves.

## CLI Journal Tail

`labby logs` is the operator-facing journal tail. It reads the `labby` systemd unit locally or, when a Labby Incus container is selected or uniquely detected, runs the journal query inside that container.

Use the generated [CLI help](../generated/cli-help.md) for current flags and arguments.

## Related Docs

- [Observability](../dev/OBSERVABILITY.md)
- [Operations](../OPERATIONS.md)
- [Incus runtime](../runtime/INCUS.md)
- [Service model](../dev/SERVICES.md)
