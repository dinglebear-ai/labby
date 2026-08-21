---
title: "Filesystem Service"
updated: "2026-08-18"
---

# Filesystem Service

The optional `fs` service exposes a jailed, read-only view of the configured workspace root. It is intended for the Labby web experience and MCP/API callers that need safe path discovery and preview without granting a general-purpose filesystem write surface.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact schemas and surface availability.

## Actions

- `fs.list` lists entries beneath the configured workspace root and reports bounded metadata.
- `fs.preview` returns a bounded file preview through HTTP/web with a safe MIME type.
- `help` and `schema` expose service discovery metadata.

There are no filesystem write, rename, delete, or arbitrary path-escape actions in this service.

## Path Safety

All requested paths are resolved against the configured workspace root and pass through the shared path-safety layer. A missing workspace configuration produces a structured `workspace_not_configured` error instead of silently browsing another directory.

## Feature Gate

The service is compiled by the `fs` feature. Builds without that feature do not advertise it.

## Related Docs

- [Configuration](../runtime/CONFIG.md)
- [Errors](../dev/ERRORS.md)
- [Service model](../dev/SERVICES.md)
