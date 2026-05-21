# Service Onboarding Review

Date: 2026-04-08

Scope:

- `radarr`
- `bytestash`
- `unifi`

This report reviews the current implementation pattern across the three services that are already online enough to evaluate as onboarding examples.

## Terminology

In this report, `HTTP API` refers to the product HTTP surface under `crates/lab/src/api/`.

That is the same product surface sometimes previously referred to as `HTTP` or `API`. For this repo, `HTTP API` is the clearest term and should be preferred when discussing that surface.

## Executive Summary

The current service architecture is fundamentally sound:

- service business logic lives in `lab-apis`
- CLI, MCP, and HTTP API are thin product surfaces
- the shared error model is stable
- the new observability, error, and serialization docs now define the intended contracts clearly

The main issue is not leaked business logic. The issue is repeated surface-layer wiring and repeated client-setup logic in `crates/lab/src`.

That repetition has already produced drift:

- Radarr uses a bespoke typed CLI
- ByteStash and UniFi use an action-driven CLI that mirrors MCP
- HTTP API handlers are nearly identical copy-paste wrappers
- MCP service modules duplicate env-based client construction and common param helpers

The next round of abstraction should reduce surface boilerplate, not hide service-specific API logic.

## What Is Working Well

### SDK Boundary

The core architectural rule is holding:

- upstream request construction lives in `lab-apis`
- upstream response parsing lives in `lab-apis`
- service endpoint methods live in the SDK clients

Radarr is the best expression of this pattern because its SDK is split by resource modules instead of growing into one giant client file.

Relevant files:

- [client.rs](/home/jmagar/workspace/lab/crates/lab-apis/src/radarr/client.rs)
- [client.rs](/home/jmagar/workspace/lab/crates/lab-apis/src/bytestash/client.rs)
- [client.rs](/home/jmagar/workspace/lab/crates/lab-apis/src/unifi/client.rs)

### Shared Error Model

The services align to the shared `ApiError` taxonomy and the dispatch layers map that into stable MCP and HTTP API semantics.

Relevant files:

- [error.rs](/home/jmagar/workspace/lab/crates/lab-apis/src/core/error.rs)
- [envelope.rs](/home/jmagar/workspace/lab/crates/lab/src/mcp/envelope.rs)
- [error.rs](/home/jmagar/workspace/lab/crates/lab/src/api/error.rs)

### Thin HTTP API Surface

The HTTP API service modules are thin wrappers around the service dispatchers. That is directionally correct even though the implementation is repetitive.

Relevant files:

- [radarr.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/radarr.rs)
- [bytestash.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/bytestash.rs)
- [unifi.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/unifi.rs)

## Findings

### 1. HTTP API handlers are duplicated almost verbatim

The Radarr, ByteStash, and UniFi HTTP API handlers all follow the same structure:

- accept `ActionRequest`
- call the matching MCP dispatcher
- time the call
- emit a dispatch log
- wrap the JSON response

Files:

- [radarr.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/radarr.rs#L12)
- [bytestash.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/bytestash.rs#L12)
- [unifi.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/unifi.rs#L12)

This is not a correctness bug, but it is pure onboarding friction. Every new service currently needs a hand-written copy of the same adapter.

### 2. HTTP API state is present but not actually used

Each HTTP API handler receives `State(_state): State<AppState>` and ignores it.

Files:

- [radarr.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/radarr.rs#L13)
- [bytestash.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/bytestash.rs#L13)
- [unifi.rs](/home/jmagar/workspace/lab/crates/lab/src/api/services/unifi.rs#L13)

That means the HTTP API surface is not yet using app state as the canonical dependency source. It also means env-based client construction remains embedded in other surface layers.

### 3. Client construction is duplicated inside MCP service modules

Each service MCP dispatcher builds its own client from env:

- Radarr: historical `crates/lab/src/mcp/services/radarr.rs`
- ByteStash: historical `crates/lab/src/mcp/services/bytestash.rs`
- UniFi: historical `crates/lab/src/mcp/services/unifi.rs`

This has a few costs:

- repeated boilerplate for every new service
- inconsistent instance-handling evolution
- harder symmetry between CLI, MCP, and HTTP API
- surface code owns too much setup logic

This is still surface logic, not service business logic, but it is a strong candidate for centralization.

### 4. CLI shape has already drifted across services

Radarr uses a bespoke typed subcommand tree:

- [radarr.rs](/home/jmagar/workspace/lab/crates/lab/src/cli/radarr.rs#L18)

ByteStash and UniFi use free-form `action` plus `key=value` params that mirror MCP:

- [bytestash.rs](/home/jmagar/workspace/lab/crates/lab/src/cli/bytestash.rs#L14)
- [unifi.rs](/home/jmagar/workspace/lab/crates/lab/src/cli/unifi.rs#L14)

This makes onboarding ambiguous. A new service author has to decide which CLI style to follow, and that decision is not yet codified as a rule.

### 5. Action-style CLI param parsing is duplicated

ByteStash and UniFi both implement the same `parse_params` and `coerce_value` helpers:

- [bytestash.rs](/home/jmagar/workspace/lab/crates/lab/src/cli/bytestash.rs#L43)
- [unifi.rs](/home/jmagar/workspace/lab/crates/lab/src/cli/unifi.rs#L43)

This is low-risk duplication and should be extracted immediately.

### 6. MCP service modules are becoming large monoliths

UniFi and Radarr in particular are already large action-switch modules:

- historical `crates/lab/src/mcp/services/unifi.rs`
- historical `crates/lab/src/mcp/services/radarr.rs`

This is survivable for a while, but it is the next likely maintainability pressure point. Radarr’s SDK already solved this pattern well by splitting endpoint logic by resource. MCP may eventually need a lighter version of that organization.

## What Is Not The Problem

The main problem is not that service business logic has leaked into `crates/lab/src`.

The current problem is:

- repeated surface logic
- repeated client-resolution logic
- inconsistent CLI shape
- repeated dispatch helpers

That distinction matters. The fix is not “move everything into `lab-apis`.” The fix is to centralize the repeated boundary code inside `lab`.

## Recommended Abstractions

### 1. Shared HTTP API dispatch adapter

Add a helper in `crates/lab/src/api/` that owns:

- action request unpacking
- timing
- dispatch logging
- request ID propagation
- JSON response wrapping

Then each service HTTP API module becomes a thin registration file instead of a copy-pasted handler.

### 2. Shared client resolver in `lab`

Add a central resolver for configured clients and instances.

It should own:

- env lookup
- instance selection
- auth shape construction
- client construction

Then CLI, MCP, and HTTP API all resolve clients the same way instead of rebuilding that logic per service module.

### 3. Shared action-style CLI param parser

Add one helper for:

- `Vec<String>` to `serde_json::Value`
- bool / integer / float coercion
- invalid `key=value` diagnostics

That immediately removes duplication between ByteStash and UniFi and gives future action-style CLIs a standard path.

### 4. Shared dispatch helper for CLI, MCP, and HTTP API

Add one helper that owns:

- dispatch timing
- caller-context span creation
- common logging shape
- error-to-envelope shaping where applicable

This is especially important now that observability is a formal contract.

### 5. CLI strategy rule

Document and enforce when a service should use:

- a typed CLI subcommand tree
- an action-style CLI that mirrors MCP

Recommended default:

- use action-style CLI for broad or unstable surfaces
- use typed subcommands only for small, stable, operator-friendly service surfaces

That rule should live in the onboarding guidance so new services do not pick a style ad hoc.

### 6. Optional MCP organization by resource

If UniFi and future services continue growing, split MCP service modules by resource or action group.

That should not change the one-tool-per-service public model. It is only an internal maintainability improvement.

### 7. Service completion checker

Add a command or script such as:

```bash
just service-check <name>
```

It should verify:

- feature flags are wired in both crates
- service is registered in CLI, MCP, and HTTP API
- coverage doc exists
- canonical docs are updated when required
- the service compiles under its feature flag

This would prevent omission-based onboarding errors better than prose alone.

## Recommended Implementation Order

1. Extract shared action-style CLI param parsing
2. Add a shared HTTP API dispatch adapter
3. Add a shared client resolver and instance lookup layer
4. Add a shared CLI/MCP/HTTP API dispatch helper for observability and timing
5. Add `just service-check <name>`
6. Revisit large MCP service-module organization if file growth continues

## Suggested Doc Updates

The current onboarding docs are much stronger than before, but they could still become more concrete in two places:

- explicitly document the CLI-style decision rule
- explicitly say that client construction should prefer shared resolution helpers over per-service env reads when such helpers exist

## Closing View

The current onboarding flow is already understandable. What it lacks is not conceptual clarity, but mechanical leverage.

The next abstractions should focus on removing repeated boundary code:

- repeated handler wrappers
- repeated env/client setup
- repeated CLI parsing
- repeated dispatch instrumentation

That will make new services faster to add without hiding the parts that actually need to remain explicit: SDK endpoint methods, service types, and service-specific action catalogs.
