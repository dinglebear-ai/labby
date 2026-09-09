---
title: "Service Model"
created: "2026-07-30"
updated: "2026-09-07"
---

# Service Model

Labby registers a small product catalog over one shared dispatch contract. The
generated [service catalog](../generated/service-catalog.md) is authoritative.

## Current Services

| Service | Exposure | Surfaces | Ownership |
| --- | --- | --- | --- |
| `access` | always on (caller-bound) | MCP, API, web | Teams, memberships, invitations, project assignments, platform administrators, Team Gateway credential bindings |
| `agents` | always on (caller-bound) | MCP, API, web | Owner-scoped Agent definitions, sessions, and execution |
| `artifacts` | feature-gated (`skills`) | CLI, MCP, API, web | Durable local Skill Library plus bounded provider-backed discovery and control-plane projections |
| `browser` | always on | MCP, API, web | Rust-native WebMCP browser bridge, pairing, discovery, consent, invocation |
| `bundles`, `jobs`, `sources`, `uploads` | feature-gated (`skills`) | MCP, API, web | Remote Depot control-plane authority services |
| `dev_containers` | always on (caller-bound) | MCP, API, web | Owner-scoped Dev Container definitions, leases, recovery, lifecycle |
| `doctor` | always on | CLI, MCP, API | Supported configuration and runtime diagnostics |
| `fs` | feature-gated (`fs`) | MCP, API, web | Optional configured filesystem browser |
| `gateway` | feature-gated (`gateway`) | CLI, MCP, API, web | Upstreams, protected routes, virtual servers, OAuth, Loadouts, Code Mode host |
| `lab_admin` | runtime-conditional (`lab-admin`) | CLI, MCP | Explicitly enabled administrative actions |
| `projects` | always on (caller-bound) | MCP, API, web | Team-owned Project lifecycle |
| `server_logs` | always on | CLI, MCP, API | Local Labby server-log search and inspection |
| `setup` | always on | CLI, MCP, API, web | Bootstrap, provisioning, plugin hooks, host service |
| `snippets` | feature-gated (`gateway`) | CLI, MCP, API | Code Mode snippet storage and execution metadata |
| `stash` | Linux only (caller-bound) | MCP, API, web | Principal- or Team-scoped File Stash |
| `tasks` | always on (caller-bound) | MCP, API, web | Durable owner-scoped Agent Tasks, admission, execution, revocation |

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
- Destructive classification is shared across surfaces.
- Service metadata drives generated catalogs and help.
- Feature-gated services must compile in their documented slices.
- Runtime-conditional services remain absent unless explicitly enabled.

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
