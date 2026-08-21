---
title: "Lab Admin Service"
created: "2026-08-18"
updated: "2026-08-18"
---

# Lab Admin Service

`lab_admin` is a small runtime-conditional administrative service. It is not a general management namespace and it is not exposed through the HTTP API or web UI.

When enabled, the service is available through CLI and MCP and currently owns only the shared onboarding-audit surface plus `help` and `schema` discovery actions.

## Actions

- `onboarding.audit` audits selected service onboarding state using local repository/product metadata.
- `help` returns the service catalog.
- `schema` returns the schema for a named action.

The generated [service catalog](../generated/service-catalog.md) and [action catalog](../generated/action-catalog.md) are authoritative for whether the service is currently exposed and for its exact schemas.

## Related Docs

- [Service onboarding](../dev/SERVICE_ONBOARDING.md)
- [Service model](../dev/SERVICES.md)
- [Dispatch contract](../dev/DISPATCH.md)
