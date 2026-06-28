# Device Runtime Master Design

Date: 2026-04-15
Status: Draft
Related bead: `lab-jzzf`
Related spec: `2026-04-15-oauth-local-callback-relay`

## Summary

`lab` should not grow a separate always-on helper binary. The existing `lab serve` process becomes the long-running device runtime on every machine where Lab is installed.

Every device runs the same runtime. For v1, exactly one device is the named `master`.

The master owns all central/operator-facing surfaces and central state:

- Web UI
- API
- MCP gateway
- syslog storage/query
- fleet inventory/state

Non-master devices run the same local runtime capabilities, but they do not expose central/operator-facing surfaces. All operator actions go through the master.

For v1, this is a Linux-only, `x86_64`, single-operator homelab design. Devices on the tailnet are treated as trusted. We do not design access control, per-device permissions, or multi-tenant trust boundaries in this slice.

## Goals

- Reuse the existing `lab serve` binary/process as the device runtime
- Avoid introducing a second daemon/helper model
- Define a clean master/non-master contract inside the existing HTTP server
- Make the master the single source of truth for fleet state, logs, and debugging
- Turn OAuth callback relay into one device capability inside the broader runtime
- Centralize syslog from all devices, including the master itself

## Product Principles

- least friction possible
- zero-to-working with minimal explicit config
- prefer discovery of existing operator state over asking for new config
- piggyback on infrastructure the operator already has:
  - Tailscale
  - SSH
  - existing AI CLI configs
- keep required operator configuration in single digits
- make deployment automation a first-class UX path, not an afterthought
- keep manual setup possible without making it painful

## Non-Goals

- automatic peer discovery or leader election
- device registration or per-device trust/permission systems
- cross-platform support beyond Linux
- multi-master or failover routing
- distributed ownership of central surfaces
- direct operator control of non-master devices

## Core Model

### Runtime

Every machine runs the same `lab serve` binary. There is no separate helper executable.

`lab serve` owns:

- local background collectors/workflows for the current machine
- device-runtime endpoints under a dedicated namespace
- master-only central surfaces when the device is configured as master

### Roles

Each device is one of:

- `master`
- `non-master`

This is not a capability matrix where one class of device runs a fundamentally different process. The same runtime exists on every machine. The difference is whether the central/operator-facing surfaces are active and whether the device owns centralized fleet state.

## Bootstrap and Configuration

### Source of Truth

`config.toml` is the source of truth for explicit master configuration.

### Default Behavior

If no master is explicitly configured, the first device defaults to master.

That means the lowest-friction first-run path is:

- install/run `lab serve` on the first machine
- it becomes the master by default

Later devices point at that master and run as non-master devices.

### Master Identification

For v1, the master should be identified by Tailscale machine name or MagicDNS hostname.

Examples:

- `controller`
- `controller.tailnet-name.ts.net`

This hostname is both:

- the operator-facing master identity
- the connection target other devices use to reach the master

The local machine name is also used as the device identity for inventory and log tagging.

If a machine name changes, the operator updates config.

### Discovery

V1 does not own discovery.

We assume the operator already knows how to connect devices securely over the tailnet. The runtime does not attempt:

- automatic peer discovery
- automatic master election
- installed-but-not-running device detection

A pre-flight/install script may choose an initial default, but runtime behavior must not depend on tailnet discovery.

## Trust Model

V1 assumes a single-operator homelab.

We do not design:

- ACLs
- role-based permissions
- per-device authorization layers
- partial fleet visibility

Anything on the tailnet running `lab serve` is treated as trusted for this slice.

## HTTP Boundary

Device-runtime traffic should live on the existing `lab serve` HTTP server under a dedicated namespace:

- `/v1/device/*`

This keeps one process and one listen socket while giving us a clean protocol boundary between:

- normal operator-facing routes
- device-runtime traffic between devices and the master

V1 uses structured HTTP only. No raw syslog protocol ingestion between devices and the master.

## Control Plane

### Master Responsibilities

The master owns:

- fleet inventory state
- device last-seen/connection state
- centralized syslog/event storage
- fleet query/correlation surfaces
- fleet-scoped orchestration
- centralized debugging surface
- all operator-facing entrypoints

For v1, these central surfaces are bundled together on the master:

- API
- MCP gateway
- syslog storage/query
- Web UI

We intentionally do not make the operator configure these independently in v1.

### Non-Master Responsibilities

Non-master devices own local collection and local execution:

- collect local health/status
- collect local device metadata
- collect and buffer syslog/events
- push state and logs to the master
- execute master-requested local workflows such as OAuth callback relay

Non-master devices are not a source of fleet truth and are not intended to be operated directly.

### Operator Traffic

Everything should go through the master.

Even if a non-master device is reachable on the tailnet, the intended operator UX is:

- user talks to the master
- master talks to devices

Non-master devices should not expose central/operator-facing surfaces in v1.

### Master as Device

The master must also ingest its own local logs/status through the same logical pipeline so all devices are treated uniformly. Devices should be able to query the master's logs through the same centralized surfaces used for the rest of the fleet.

## Initial Device Capabilities

V1 device runtime capabilities on every device:

- presence and role advertisement to the master
- health and status
- basic device info:
  - CPU
  - RAM
  - storage usage
  - OS
  - IP
- basic device metadata
- OAuth callback relay execution
- syslog forwarding
- AI CLI MCP config discovery

## AI CLI MCP Config Discovery

Each device runtime should scan for installed AI CLI config files:

- `~/.claude.json`
- `~/.codex/config.toml`
- `~/.gemini/settings.json`

If present, the device should parse the user's currently configured MCP servers and report that inventory to the master as device metadata.

### V1 Boundary

This capability is discovery and reporting only.

V1 must:

- discover local AI CLI MCP configuration
- parse and report the inventory to the master
- store the reported inventory on the master per device

V1 must not:

- mutate local AI CLI config files
- sync MCP config between devices
- auto-reconcile drift

### Stored Metadata

The master should store both:

- parsed MCP server definitions
- source file metadata:
  - source file path
  - mtime
  - content hash

This is intentionally more than the immediate v1 feature needs, because it makes later drift detection and config-sync workflows much easier without meaningful storage cost.

## Syslog Model

### Direction

Syslog is device -> master push.

The master is the central fleet log store.

### Buffering

Devices must buffer locally when the master is unavailable and retry later. V1 should not be fire-and-forget.

This implies a durable device-side outbound queue for:

- syslog
- device events
- possibly future outbound runtime records

### Query Surface

The master exposes the log search/query/correlation surfaces through:

- CLI
- MCP
- API
- later, Web UI log viewer integration

The existing log viewer direction should extend naturally from “master logs” to “fleet logs by device”.

## OAuth Relay Integration

The existing `relay-local` work remains the initial implementation slice, but in the broader design it becomes one device capability hosted by the same long-running runtime.

That means:

- no separate helper binary
- relay execution happens inside the same device runtime model
- the master can invoke relay capability on a device

## Linux-Only Assumption

V1 is explicitly Linux-only.

The design may assume Linux-native behavior for:

- process/service lifecycle
- syslog/journald collection
- filesystem locations
- host metrics collection

Do not add cross-platform abstraction pressure in this slice.

For v1, we also assume `x86_64` Linux only. Do not design the first deployment workflow around multi-architecture binary selection yet.

## Recommended API Shape

Illustrative namespace only; exact routes are still implementation details:

- `/v1/device/hello`
- `/v1/device/status`
- `/v1/device/metadata`
- `/v1/device/syslog/batch`
- `/v1/device/oauth/relay/*`

The important part is not the exact route naming. The important part is:

- dedicated device-runtime namespace
- structured HTTP payloads
- one configured master target per non-master device

## Implementation Details

This section is intended to remove ambiguity for the first implementation. The implementing agent should treat these details as the default contract unless a later spec explicitly supersedes them.

### Network Topology

V1 uses a star topology.

- every non-master device connects to the master
- the master may call back into devices for device-local workflows
- non-master devices do not talk directly to each other
- operator-facing traffic goes to the master only

This is not a mesh and not peer-to-peer for v1.

### Transport

Inter-device communication should use normal HTTP between `lab serve` instances over the tailnet.

Use:

- existing `lab serve` HTTP server
- device-runtime route family under `/v1/device/*`
- structured JSON request/response payloads

Do not use for v1:

- raw syslog protocol between devices
- gRPC
- websocket-based runtime coordination
- distributed pub/sub between non-master devices

### Runtime Flows

The core v1 flows are:

#### Device -> Master

- hello / check-in
- health and status update
- device metadata update
- AI CLI MCP config inventory upload
- syslog batch upload

#### Master -> Device

- invoke device-local OAuth callback relay workflows
- later, invoke other device-local management workflows

#### Operator -> Master

- Web UI
- API
- MCP gateway
- centralized log search/query
- fleet inventory queries

Operator requests must not target non-master devices directly in v1.

### Master Configuration

The spec should assume one named master in config, not boolean flags scattered across devices.

Desired shape:

```toml
[device]
master = "controller"
```

Behavior:

- if `device.master` is absent, the first device defaults to master
- if `device.master` matches the local device identity, that device runs as master
- otherwise the device runs as non-master and connects to the configured master

Do not introduce separate config for:

- syslog owner
- API owner
- MCP owner
- Web UI owner

For v1, those all belong to the master.

### Device Identity

Use Tailscale machine name / MagicDNS hostname as the operator-facing device identity.

The runtime should:

- use the local machine name as the stable device ID for inventory/log tagging
- use the configured master hostname as the network target
- treat hostname changes as operator-visible config changes

Do not invent a second custom UUID-style device identity scheme for v1.

### Tailscale Interaction

V1 runtime must not depend on Tailscale device scanning for basic operation.

That means:

- no runtime peer discovery
- no runtime auto-election
- no requirement to enumerate tailnet devices before startup

Tailnet assumptions are limited to:

- the operator has already connected devices correctly
- devices can resolve/reach each other by Tailscale name or MagicDNS hostname

### Tailscale Scanning Direction

Tailscale scanning is allowed only as a future onboarding/deploy helper, not as a core runtime dependency.

When added, prefer:

1. `tailscale status --json`
2. local SSH config inspection

Do not add Tailscale API auth or control-plane dependencies to the first runtime slice.

### Parsing Strategy

There are three main parsing domains in v1.

#### 1. Device Runtime Payloads

Use strongly typed Rust structs with:

- `serde`
- `serde_json`

All `/v1/device/*` payloads should have explicit request/response structs. Do not rely on loose ad hoc JSON maps where a typed struct is reasonable.

#### 2. AI CLI Config Discovery

Use:

- `serde_json` for:
  - `~/.claude.json`
  - `~/.gemini/settings.json`
- `toml` for:
  - `~/.codex/config.toml`

Parsing rules:

- parse only the fields needed for MCP inventory
- ignore unknown fields
- tolerate schema drift where possible
- treat malformed files as per-file recoverable errors
- never let one malformed CLI config block device startup

#### 3. Log Collection Normalization

For v1, do not design around raw protocol-level syslog parsing between devices and the master.

Instead:

- collect locally from Linux-native log sources
- normalize records into Lab-owned structured event payloads
- upload structured batches to the master

The master should store normalized fleet log events, not raw syslog frames.

### Linux Log Collection

V1 is Linux-only, so log collection may rely on Linux-native facilities.

Preferred first target:

- journald / syslog-compatible local sources on Linux

Design requirement:

- local collection should be abstracted behind a small internal module boundary so the ingest/upload pipeline does not care which Linux log source produced the event

### Queueing Model

Each non-master device needs a durable local outbound queue for:

- syslog
- device events
- future outbound runtime records

Queue requirements:

- durable on disk
- survives restart
- supports retry when master is unavailable
- supports batched upload
- does not block normal device runtime startup

The first implementation should optimize for correctness and durability over sophistication.

### Storage Model

The master stores central fleet state:

- device inventory
- last-seen / connectivity state
- AI CLI MCP inventory per device
- centralized normalized log events

The spec intentionally leaves exact DB table/schema details to the implementation plan, but the ownership is not optional: this state lives on the master.

### Dependency Guidance

Keep dependencies minimal.

Use existing deps where possible:

- `tokio`
- `axum`
- `reqwest`
- `serde`
- `serde_json`
- `toml`
- `tracing`

Reasonable additions for v1:

- `sha2` for config-file hashing
- `sysinfo` for CPU/RAM/storage/device info if existing code does not already provide a better path

Avoid for v1:

- heavy Tailscale SDK/control-plane dependencies
- raw syslog server crates
- distributed coordination libraries
- consensus/replication tooling
- multi-master coordination dependencies

### Internal Module Direction

The implementation should keep device-runtime logic separate from existing user-facing surfaces.

Preferred structure:

- `crates/lab/src/device/`
  - `identity.rs`
  - `checkin.rs`
  - `inventory.rs`
  - `syslog.rs`
  - `queue.rs`
  - `master_client.rs`
- `crates/lab/src/api/device/`
  - master-side `/v1/device/*` routes

The exact filenames can change, but the separation of concerns should remain:

- device-local collection/execution
- master-facing client logic
- master-side device API handlers

### Error Handling Rules

V1 device-runtime features must fail narrowly.

Specifically:

- malformed AI CLI config files must not crash the runtime
- temporary master unavailability must not lose logs if they are already queued
- one failing device capability must not bring down the whole `serve` runtime

Errors should be:

- logged structurally with `tracing`
- attributable to a device and capability
- visible from the master for debugging where possible

### Operational Default

The implementing agent should optimize for:

- the fewest required config knobs
- the smallest number of moving parts
- one obvious operator entrypoint: the master
- one obvious runtime transport: structured HTTP over tailnet

Do not “future-proof” v1 by introducing extra config surfaces or generalized distributed abstractions that are not required by this spec.

## Implementation Touch Points

This section maps the design onto the current repository so the implementing agent does not have to guess where work belongs.

### Existing Rust Files to Modify

#### Runtime and startup

- `crates/lab/src/config.rs`
  - add device/master configuration model
  - define master defaulting rules
  - keep config surface minimal

- `crates/lab/src/cli/serve.rs`
  - start device runtime inside `serve`
  - activate master-only central surfaces or non-master outbound runtime behavior

- `crates/lab/src/main.rs`
  - add top-level module wiring if new device runtime modules are introduced

#### HTTP API wiring

- `crates/lab/src/api.rs`
  - module declarations for any new device API surface

- `crates/lab/src/api/router.rs`
  - mount `/v1/device/*`
  - gate operator-facing central surfaces to master-only mode

- `crates/lab/src/api/state.rs`
  - add shared runtime state for:
    - local device runtime
    - master-side fleet state/storage handles
    - queue/log/inventory dependencies

- `crates/lab/src/api/web.rs`
  - make Web UI master-only in v1

- `crates/lab/src/api/openapi.rs`
  - update OpenAPI if `/v1/device/*` is documented there

#### Existing OAuth integration points

- `crates/lab/src/oauth/local_relay.rs`
  - reuse as the device-local OAuth relay capability

- `crates/lab/src/oauth/target.rs`
  - reuse if target resolution remains useful for device-invoked relay execution

- `crates/lab/src/cli/oauth.rs`
  - keep current operator flow coherent with the broader device runtime direction

#### MCP / catalog / runtime behavior

- `crates/lab/src/catalog.rs`
  - add any new fleet/device/log surfaces that should appear in product help/catalog output

- `crates/lab/src/registry.rs`
  - wire any new MCP-visible fleet/log/device tooling if introduced

- `crates/lab/src/mcp/server.rs`
  - gate master-only MCP surfaces as needed

- `crates/lab/src/mcp/resources.rs`
  - update if fleet/device/log resources are exposed

#### CLI wiring

- `crates/lab/src/cli.rs`
  - register any new top-level CLI groups added by this feature set

### New Rust Modules to Create

These should be added as a new product-local subsystem rather than folded into `dispatch/gateway/`, which already owns the upstream MCP gateway control plane.

#### Device runtime

- `crates/lab/src/device.rs`
- `crates/lab/src/device/identity.rs`
- `crates/lab/src/device/runtime.rs`
- `crates/lab/src/device/master_client.rs`
- `crates/lab/src/device/checkin.rs`
- `crates/lab/src/device/inventory.rs`
- `crates/lab/src/device/logs.rs`
- `crates/lab/src/device/queue.rs`
- `crates/lab/src/device/oauth.rs`

These files cover:

- local device identity
- master/non-master resolution
- local runtime orchestration
- device -> master client calls
- check-in/status payloads
- AI CLI config discovery
- local log collection/normalization
- durable outbound queue
- bridge into existing OAuth relay behavior

#### Master-side device API

- `crates/lab/src/api/device.rs`
- supporting files under `crates/lab/src/api/device/` such as:
  - `hello.rs`
  - `status.rs`
  - `metadata.rs`
  - `syslog.rs`
  - `oauth.rs`

The exact filenames may vary, but the implementation must keep the concerns separate:

- device-local runtime logic
- device -> master client logic
- master-side device HTTP handlers

### Existing Docs to Update

- `docs/README.md`
  - add the device runtime/master model to the docs index

- `docs/CONFIG.md`
  - document the new device/master config
  - clarify defaulting behavior

- `docs/CLI.md`
  - add any new device/log/deploy commands

- `docs/OAUTH.md`
  - explain how OAuth relay fits into the broader device runtime model

- `docs/OPERATIONS.md`
  - add master/non-master operational guidance
  - document debugging flow from the master

- `docs/TRANSPORT.md`
  - add `/v1/device/*`
  - clarify master-only operator-facing surfaces

- `docs/MCP.md`
  - update if fleet/log/device MCP tools are added

- `docs/ARCH.md`
  - update architecture ownership and the master-based runtime model

- `docs/OBSERVABILITY.md`
  - add device-runtime logs, master/device correlation, syslog ingest logging, queue/upload observability

- `docs/ERRORS.md`
  - add any new device-runtime-specific stable error kinds

- `docs/GATEWAY.md`
  - add cross-reference or wording guardrails so the existing upstream MCP gateway feature is not confused with the new `master` role

### New Docs to Create

- `docs/DEVICE_RUNTIME.md`
  - canonical doc for the master/non-master runtime model

- `docs/FLEET_LOGS.md`
  - canonical doc for centralized log ingest/search/correlation by device

- `docs/DEPLOY.md`
  - deployment/onboarding flow once `lab deploy` begins landing

### API Endpoints to Add

#### Device -> master endpoints

- `POST /v1/device/hello`
  - initial check-in and identity/role/version bootstrap

- `POST /v1/device/status`
  - periodic health/status updates

- `POST /v1/device/metadata`
  - device metadata and AI CLI MCP inventory upload

- `POST /v1/device/syslog/batch`
  - batched normalized log ingest

#### Master -> device workflow endpoints

- a device-local OAuth relay invocation route under `/v1/device/oauth/*`
  - exact naming may vary
  - must remain within the `/v1/device/*` namespace

#### Master-only read/query surfaces

These may be implemented in the product API namespace rather than `/v1/device/*`, but they must be master-only:

- a fleet inventory endpoint, e.g. `GET /v1/devices`
- a per-device detail endpoint, e.g. `GET /v1/devices/:id`
- a centralized log search endpoint, e.g. `POST /v1/logs/search`
- a per-device log query endpoint, e.g. `GET /v1/logs/devices/:id`

### Existing API Surfaces to Modify

- `/health` and `/ready`
  - consider whether readiness should reflect master/device-runtime state

- Web UI asset serving
  - master-only in v1

### CLI Surfaces to Add or Modify

Likely additions:

- `lab device ...`
- `lab logs ...`
- `lab deploy <device>`

Likely modified:

- `lab serve`
- `lab oauth ...`

### Web UI Areas Likely Affected

The first runtime slice may not need full Web UI work, but the design already implies future touch points:

- `apps/gateway-admin/app/(admin)/gateway/page.tsx`
- `apps/gateway-admin/app/(admin)/gateways/*`
- future fleet/device pages
- future fleet log viewer work extending the existing gateway log viewer direction

### Important Terminology Rule

The repository already uses `gateway` extensively for the upstream MCP gateway feature:

- `dispatch/gateway/`
- `/v1/gateway`
- `docs/GATEWAY.md`

The implementing agent must not overload that terminology for this feature.

Use:

- `master` for the central device-runtime authority
- `device runtime` for the new subsystem

Do not use:

- `gateway` to mean the master role in new code or docs

## V1 Constraints

- exactly one named master
- no multi-master fanout
- no failover design yet
- no dynamic discovery protocol
- no access-control layer
- no direct operator control of non-master devices

## Operational Consequences

This design intentionally makes debugging converge on the master.

When something goes wrong, the operator should be able to start at the master and inspect:

- whether the device is connected
- when it was last seen
- what status it last reported
- whether syslog is arriving
- whether relay/device actions failed

That is the main reason all fleet operations and logs route through the master.

## Low-Friction Deployment Direction

Once a master is up and running, the product should be able to piggyback on the operator's existing SSH setup instead of requiring lots of manual device configuration.

The intended direction is:

- inspect `~/.ssh/config`
- identify candidate devices
- test SSH reachability
- allow `lab deploy <device>`
- copy or sync the existing Lab binary to the remote host
- write minimal device config pointing at the master
- start or enable `lab serve` on the remote machine
- verify that the device checks in to the master

This deployment flow should eventually be exposed through:

- CLI
- MCP
- API
- Web UI

### V1 Boundary for Deploy

This spec does not require the full deployment flow to land in the first runtime slice. It does establish the intended low-friction direction and the assumptions that make it feasible:

- Linux-only
- `x86_64` only
- SSH already works
- tailnet connectivity already works

The deployment workflow is an operator accelerator, not a replacement for having a simple runtime contract.

## Follow-On Work

Likely follow-on design and implementation areas after v1:

- explicit master config schema in `config.toml`
- device-runtime HTTP namespace and payload contracts
- durable device-side queue for syslog/events
- master-side fleet inventory persistence
- centralized syslog storage and query surfaces
- fleet log correlation and Web UI log viewer integration
- turning relay-local into a master-mediated device capability
- MCP config reconciliation and sync flows across discovered AI CLI configs
