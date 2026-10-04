---
title: "Service Model"
created: "2026-07-30"
updated: "2026-09-29"
---

# Service Model

Labby registers a small product catalog over one shared dispatch contract. The
generated [service catalog](../generated/service-catalog.md) is authoritative.

## Catalog, Registration, And Exposure

Use the generated [service catalog](../generated/service-catalog.md) for exact
service names, feature classifications, metadata owners, and supported surfaces.
Use the [service documentation index](../services/README.md) for behavioral
contracts and the [action catalog](../generated/action-catalog.md) for action
metadata. This guide explains the model rather than maintaining another copy
of those inventories.

The all-feature documentation projection uses `build_docs_registry` and includes
runtime-conditional definitions. A catalog entry is not proof that a service is
registered in a running process or exposed to a particular caller. Keep these
checks separate:

- compilation selects Cargo features;
- runtime registration applies startup and platform conditions, including the
  admin opt-in and Linux-only File Stash;
- route and surface projections select what is visible and callable;
- caller identity, Team/project authority, scopes, and action-specific checks
  determine whether the requested operation is authorized.

Not every catalog entry represents a registered multi-surface service. The
`proxy` entry describes the direct stdio-proxy CLI runtime; protected-route
publication has its own `artifact_publish` contract. Neither should be inferred
from a hand-maintained list of ordinary context-free dispatchers.

Caller-bound services need a host-established identity and transport
authority ceiling. Their registry entries answer only `help`/`schema`; every
other action is bound by the authenticated HTTP adapter or the MCP
caller-bound path. See
[../access-control/MULTI_USER_AUTHORITY.md](../access-control/MULTI_USER_AUTHORITY.md).

## Registration Rules

A first-class service has one canonical action catalog and one shared dispatcher.
CLI, MCP, HTTP, and web code are adapters over that dispatcher, not separate
implementations.

- Action metadata lives in `ActionSpec`/`ParamSpec`.
- `ActionSpec.requires_admin` and `destructive` are independent; use shared surface policy for authorization and confirmation rather than treating every mutation as destructive.
- Service metadata drives generated catalogs and help.
- Feature-gated services must compile in their documented slices.
- Runtime-conditional availability follows registry, platform, and startup rules, not only Cargo feature selection.

Reusable gateway, auth, Code Mode, web, and runtime behavior belongs in the
extracted `labby-*` crates. Product dispatch and configuration adapters belong
in `crates/labby`. Pure setup/doctor contracts may live in `labby-apis`.

## Adding A Service

Add only the surfaces the capability actually supports:

1. define shared metadata and typed request/result contracts;
2. implement one dispatcher;
3. register it in the product service registry;
4. add thin CLI/MCP/API/web adapters as needed;
5. regenerate catalogs and add architecture tests;
6. document config, errors, observability, and destructive behavior.

Do not add an empty Cargo feature as a placeholder for a future product.

## Retired Services

ACP, ACP Registry, the in-product MCP Registry browser/client, Marketplace,
Fleet/device runtime, Deploy-product, and the old Agent Artifact Manager named
Stash are not current services or SDK modules. The principal-scoped File Stash
is a distinct current Linux service. Historical implementation contracts are archived under
[../archive/retired-labby](../archive/retired-labby/).
