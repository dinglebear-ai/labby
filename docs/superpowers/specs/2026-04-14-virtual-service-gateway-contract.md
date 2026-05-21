# Virtual Service Gateway Contract

**Date:** 2026-04-14

## Goal

Define the product and backend contract for treating onboarded Lab services as virtual gateway servers inside Labby.

For this design, Lab-owned services such as `plex`, `unraid`, `overseerr`, and similar integrations should appear and behave like regular MCP gateway servers from the operator's point of view. The immediate goal is not to build service-specific dashboards or richer API workflows. The goal is to make service onboarding, enablement, and MCP exposure management feel identical to managing any other server in the gateway.

## Scope

This contract covers:

- Lab-owned services as virtual servers in the gateway
- unified add-server flow for Lab services and custom gateways
- canonical config persistence for service credentials
- explicit server enablement after config
- per-surface exposure controls for `CLI`, `API`, `MCP`, and `WebUI`
- MCP management parity between Lab-backed virtual servers and proxied external servers
- action-level filtering for Lab services that expose one tool with many `action` values

This contract does not cover:

- service-native operational dashboards
- rich service workflows such as approving requests or inspecting media libraries
- extract-driven onboarding automation
- redesign of the generic upstream proxy runtime for unrelated external servers

## Product Decision

The gateway server list is the only top-level abstraction.

There is no separate visible concept of "managed services" or "Lab services" in the main IA. A configured and enabled Lab-owned service materializes as a server entry in the same list as a custom HTTP or stdio MCP gateway.

The distinction between a Lab-backed virtual server and a custom proxied gateway is internal. Operators should experience them as the same product object:

- each has a server row
- each has a server detail page
- each can be enabled or disabled
- each can expose MCP capabilities
- each can show warnings, health, and connection state

The only deliberate UX difference is that a Lab-backed virtual server may later deepen into a richer service-specific page. That richer page is out of scope for this contract.

## Terminology

### Service Config

Canonical service connection information already understood by Lab's existing surfaces, typically backed by `{SERVICE}_URL`, `{SERVICE}_API_KEY`, `{SERVICE}_TOKEN`, and related config/env settings.

Examples:

- `PLEX_URL`
- `PLEX_TOKEN`
- `UNRAID_URL`
- `UNRAID_API_KEY`

### Virtual Server

A gateway-visible server record representing one Lab-owned service as an MCP-manageable server. A virtual server does not own canonical credentials. It references service config and adds gateway-facing state such as enablement and surface exposure.

### Custom Gateway

An existing external MCP server configured directly through HTTP or stdio transport.

### Surface Exposure

Whether a configured service is enabled for a given Lab runtime surface:

- `CLI`
- `API`
- `MCP`
- `WebUI`

## Core Model Contract

Lab-backed virtual servers and custom gateways must share one list-level view model, but they do not need to share identical persistence internals.

### Required separation of concerns

Canonical service credentials must not be duplicated into virtual-server state.

The system must distinguish between:

1. service connection config
2. server enablement and policy

This is a hard contract because the same service credentials should remain usable by the rest of Lab even if the service is not currently enabled as a server in the gateway.

### Canonical ownership

Service credentials are canonical in Lab config and `.env`.

Virtual server state is canonical for:

- whether the service is enabled as a server
- which surfaces are enabled
- MCP exposure policy
- operator-facing warnings specific to server enablement and exposure

## Visibility Contract

A Lab-owned service should appear in the server list once the user has provided enough config for the service to be potentially connectable.

This means "configured" and "enabled as server" are separate states, but both remain visible in the same list.

Required behavior:

- after saving service credentials, the service appears in the main server list even if it is not yet enabled as a server
- configured-but-disabled services should render as visibly inactive, for example greyed out or otherwise clearly marked
- once enabled, the same server entry transitions into an active server
- disabling the virtual server keeps the server row visible but marks it disabled
- deleting the virtual server must not implicitly delete canonical service credentials unless the user explicitly requests that behavior in a later design

For this contract, disablement should mean "visible but intentionally inactive," not "hidden" and not "forgotten."

### List filtering contract

The server list must support filtering at least by:

- `active`
- `configured`
- `enabled`
- `disabled`
- `connected`
- `disconnected`

These filters apply to both custom gateways and Lab-backed virtual servers through the same list model.

## Add Server Flow Contract

The add-server flow must become a two-tabbed flow:

- `Lab Gateways`
- `Custom Gateways`

### Lab Gateways tab

This tab presents supported Lab services as icon tiles in a selection grid.

Examples:

- Plex
- Unraid
- Overseerr
- Synapse
- Tautulli

Selecting a tile opens a service-specific config step driven by that service's declared config requirements.

Minimum required behavior:

- fields come from service metadata rather than hardcoded one-off UI logic where possible
- the form asks for the service URL and auth material required for that service
- save writes canonical service config into Lab-owned config storage and `.env`
- the flow can test connectivity before or during enablement
- the flow ends with explicit "Enable server" confirmation

On successful enablement:

- the virtual server is created
- the server appears in the gateway server list
- enabled surfaces become active according to the chosen defaults or toggles
- MCP exposure becomes manageable from the normal server detail page

### Custom Gateways tab

This tab preserves the current custom upstream gateway behavior:

- HTTP MCP servers
- stdio MCP servers

The contract here is continuity, not redesign.

## Service Metadata Contract

To support the `Lab Gateways` add flow, each onboarded service must expose enough metadata for the UI and backend to drive setup consistently.

At minimum, metadata must describe:

- service key and display name
- icon asset or icon identifier
- required config fields
- optional config fields
- auth type expectations
- which surfaces are supported
- whether MCP enablement is supported

This contract should build on the existing `PluginMeta` and env metadata patterns rather than inventing an unrelated registry if the current metadata can be extended cleanly.

## Surface Exposure Contract

Every virtual server detail page must show surface state for:

- `CLI`
- `API`
- `MCP`
- `WebUI`

Each surface must support two concepts:

- `enabled`
- `connected` or otherwise operationally ready

Examples:

- a service may be configured and enabled for `MCP` but disabled for `CLI`
- a service may be enabled for `API` but disconnected because its URL or auth is invalid
- `WebUI` may remain disabled even when the control-plane page exists, because service-specific UI features are not yet enabled

### Immediate scope

Per the current product direction, these surface indicators and toggles are in scope now because they provide immediate operator value and establish the future control-plane model.

### Minimum semantics

- `enabled` is operator intent
- `connected` is runtime health/readiness
- a disabled surface must not present as connected
- a configured but disabled surface must show that it is intentionally off, not broken

## MCP Parity Contract

Virtual servers must behave as much like regular MCP servers as possible at the gateway management layer.

Required parity includes:

- list visibility
- detail page shell
- warnings/status rendering
- enable/disable operations
- exposure controls
- test/reload style operator actions where applicable

Where exact parity is not possible because a Lab service is internally implemented rather than proxied, the operator-facing result should still be equivalent.

The product contract is parity of behavior, not parity of internal transport.

## Action Filtering Contract

Most Lab services expose exactly one MCP tool named after the service, with many `action` values behind it. Because of that, "partial exposure" for a Lab virtual server cannot stop at tool-level filtering.

The system must support filtering within a single tool by action name.

### Required policy shape

For Lab-backed virtual servers, MCP exposure policy must be able to express:

- whole-service enabled or disabled
- whole-tool enabled or disabled
- action-level allowlist or denylist within the service tool

Examples:

- expose all `plex` actions
- expose only `plex` actions matching a selected subset
- expose `overseerr` as a server but filter out administrative actions

### Enforcement contract

Action filtering must be enforced server-side in the actual tool call path.

UI-only filtering is not sufficient.

Discovery/help/schema behavior must also reflect the effective exposure policy as closely as practical so operators and downstream clients are not misled.

### Complexity note

This is the trickiest part of the design. It is still in scope, but the implementation must explicitly handle:

- help/schema visibility for filtered actions
- execution-time rejection for disallowed actions
- stable operator messaging when a filtered action is attempted

## Config Persistence Contract

When an operator configures a Lab-owned service from the `Lab Gateways` flow, the system must write canonical service configuration to Lab-owned config storage.

For current Lab architecture, that means writing the service's normal config representation, including `.env` values where that is already the canonical path.

This is preferred over storing service credentials only inside the web UI or only inside a gateway-specific record.

### Requirements

- credentials written by the add flow become available to existing Lab runtime paths
- no shadow-only credential store for the same service
- virtual server creation references canonical config rather than copying the secret values into a second persistent home
- updates made from the virtual server detail page must update canonical service config, not just the gateway view model

## Server Detail Page Contract

A Lab-backed virtual server detail page must initially remain a control-plane page, not a rich service dashboard.

In-scope elements:

- service identity and status
- surface enablement and connection state
- service config edit path
- connection test results
- MCP exposure controls
- warnings and diagnostics

Out-of-scope elements:

- service business data
- domain-specific workflows
- media requests, playback sessions, user administration, and similar operational features

The future contract allows a service page to deepen into a richer experience later, but that must not be required for this phase.

## Derived Runtime Behavior

Once a Lab service is both configured and enabled as a virtual server:

- the server must participate in the gateway server list
- its `MCP` surface can expose tools, prompts, and resources
- its `CLI` surface can be reported as enabled or disabled
- its `API` surface can be reported as enabled or disabled
- its `WebUI` surface can be reported as enabled or disabled

The runtime should not pretend that all surfaces are active simply because canonical credentials exist.

Surface state must be explicit.

## Backend Contract Shape

This design implies three distinct backend capabilities:

1. service configuration management
2. virtual server management
3. MCP exposure policy management

The current gateway management surface already owns the third category for custom upstreams. This contract extends that surface so it can also manage Lab-backed virtual servers.

### Required backend view model properties

At the list/detail level, every server should expose enough data for the UI to render a unified control plane:

- stable server id
- display name
- server kind, internal only if needed
- config summary safe for UI rendering
- enabled state
- connection state
- per-surface state for `CLI`, `API`, `MCP`, `WebUI`
- warnings
- MCP discovery and exposure summary

The frontend should not need to infer these states from raw env vars or low-level dispatch internals.

## Error Handling Contract

Operator-visible flows must distinguish:

- missing configuration
- invalid credentials
- surface intentionally disabled
- connection failure
- MCP exposure policy denial

These states must not collapse into one generic "not working" status.

### Examples

- a service with saved credentials but no enabled virtual server should read as "configured, not enabled as server"
- a disabled `CLI` surface should read as intentionally off
- a filtered MCP action should fail with a policy-denied message, not an internal error

## Security and Secrets Contract

The UI must continue to respect the existing project rules around secret handling:

- tokens and API keys are written to canonical config storage
- raw secrets are never echoed back in normal list/detail payloads
- UI models should expose presence, status, and validation outcomes rather than secret values
- service config editing must preserve current redaction and observability rules

This contract does not authorize duplicating secrets into server-list payloads or browser-local persistence.

## Migration Contract

This work should be delivered as an additive expansion of the gateway model rather than as a destructive rename of existing custom gateway behavior.

Preferred migration shape:

1. introduce service-backed virtual server concepts
2. add the two-tab add flow
3. add canonical service-config writes
4. add explicit server enablement
5. add per-surface controls
6. add action-level MCP filtering for Lab-backed virtual servers

Existing custom gateways must keep working during the migration.

## Non-Goals

The following are explicitly deferred:

- service-specific dashboards
- using the virtual server page to browse or operate the service domain itself
- extract-assisted onboarding wizard
- auto-materializing every configured service into the server list without explicit enablement

These may be future product layers, but they are not part of this contract.

## Verification

The contract is satisfied only when all of the following are true:

- a supported Lab service can be configured from the `Lab Gateways` tab
- the flow writes canonical service config to Lab-owned config storage
- the service does not appear in the server list until explicitly enabled as a server
- once enabled, it appears in the same server list as custom gateways
- the detail page shows `CLI`, `API`, `MCP`, and `WebUI` enabled/connected indicators
- the `MCP` surface can be exposed, disabled, or filtered at action level for Lab-backed services
- action filtering is enforced at runtime, not only in the UI
- disabling the virtual server removes server behavior without destroying canonical credentials

## Follow-On Work

This contract should feed at least these implementation slices:

- domain model and persistence changes for virtual servers and per-surface state
- metadata-driven `Lab Gateways` add flow
- canonical config write/update path from Labby
- unified server list/detail view model for custom and Lab-backed servers
- runtime enforcement of action-level MCP filtering
