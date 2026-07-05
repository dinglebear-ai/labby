# Node Runtime Contract

This document defines the target runtime and artifact contract for `lab` controller and node processes.

The current implementation is only partway there. It already has a WebSocket-first node runtime and route-gates several controller-only HTTP surfaces, but it still builds one all-features artifact and still initializes controller-side startup machinery in non-controller processes. The implementation must converge on this contract before node rollouts are considered production-ready.

## Roles

Every `labby serve` process runs in exactly one role:

- `controller`: owns the operator control plane and fleet coordination.
- `node`: runs on managed machines and connects outbound to the controller.

Target role selection is resolved in this order:

1. explicit CLI flag, for example `labby serve --role node`
2. config value, for example `[node].role = "node"`
3. hostname comparison against `[node].controller`

If role resolution is ambiguous, `labby serve` must fail with a clear configuration error rather than starting the full controller surface accidentally.

Current implementation note:

- `labby serve` does not yet have `--role`.
- `[node].role` does not yet exist.
- `serve` currently resolves role from local hostname compared with `[device].master` (legacy config key); if no master is configured, the local host resolves as `DeviceRole::Master` (Rust enum variant).
- `[node].controller` exists for newer node/deploy paths, but it is not yet the `serve` role selector.

## Controller Contract

The controller process may run the full product surface:

- Gateway and upstream MCP client discovery
- MCP server
- Web UI
- full `/v1/*` HTTP API
- MCP Registry and Marketplace
- fleet WebSocket listener at `GET /v1/nodes/ws`
- node inventory, enrollment, command routing, log search, and controller-side health

The controller owns durable enrollment state and all operator-visible fleet state.

Controller HTTP routes include:

- `GET /health`
- `GET /ready`
- `GET /v1/nodes/ws`
- `GET /v1/nodes`
- `GET /v1/nodes/{node_id}`
- `GET /v1/nodes/enrollments`
- `POST /v1/nodes/enrollments/{node_id}/approve`
- `POST /v1/nodes/enrollments/{node_id}/deny`
- full service routes under `/v1/{service}`
- `/mcp` when HTTP MCP transport is enabled

## Node Contract

A node process must be small and outbound-first.

Node mode starts only:

- node identity and role resolution
- local node runtime queue
- local metadata collection
- outbound WebSocket client to the controller
- command execution handlers required by controller-initiated node RPC
- optional local-only health readiness endpoints

Node mode must not start:

- Gateway manager
- upstream MCP discovery
- MCP server
- Web UI asset discovery or build
- full `/v1/*` API router
- MCP Registry
- Marketplace
- OAuth browser/operator runtime
- service integration registry for controller-facing tools

Current implementation note:

- Non-controller route mounting blocks many controller HTTP surfaces, including service routes, gateway APIs, Web UI serving, OAuth metadata, and `/mcp`.
- `labby serve` still uses one startup path for both roles. Before route gating, non-controller processes can still build the default registry, resolve controller auth, initialize upstream OAuth runtime, run upstream gateway discovery, install a gateway manager, check or build Web UI assets, and open marketplace registry state.
- The implementation target is to move those initializations behind a controller-only branch.

The node process communicates with the controller by opening an outbound WebSocket to:

```text
GET /v1/nodes/ws
```

After connection, the node sends JSON-RPC messages for:

- node initialization and enrollment status
- metadata upload
- status upload
- command result and command output frames

The node must queue outbound metadata and status locally before delivery. Queue entries are acknowledged only after the controller accepts the corresponding WebSocket message.

## Node HTTP Surface

Target node mode does not expose the full API.

If HTTP is enabled on a node, it is restricted to local health only:

- `GET /health`
- `GET /ready`

Node health HTTP must bind to `127.0.0.1` unless a future contract explicitly allows remote node health.

No node-local route may expose fleet inventory, enrollments, service dispatch, marketplace, MCP registry, OAuth, gateway, or file-system tooling.

Current implementation note:

- Non-controller HTTP is not health-only today.
- `/v1/nodes/*` is mounted in the shared router and includes compatibility routes such as `/v1/nodes/hello`, `/v1/nodes/status`, `/v1/nodes/metadata`, and `/v1/nodes/oauth/relay/start`.
- Some controller-only routes fail closed on non-controller nodes, but the routes still exist.
- Node health uses the shared `labby serve` host binding today; it is not forced to `127.0.0.1` yet.

## Build Artifacts

Target controller and node deployments use different Cargo feature sets.

Controller artifact:

- built with the controller feature profile
- includes the operator control plane and full configured service surface
- used only for the controller host

Node artifact:

- built with the node feature profile
- excludes controller-only surfaces
- used for all non-controller hosts

`nodes update --all` must build or reuse the correct artifact for each role:

- controller target receives the controller artifact
- remote node targets receive the node artifact

The deploy build step must skip rebuilding an artifact when the existing binary is newer than the build inputs for that artifact profile.

The deploy timeout budget starts after the required artifact is built or selected for reuse. Build time is not part of the rollout execution timeout.

Current implementation note:

- `crates/lab/Cargo.toml` currently has `default = ["all"]`.
- There is no `controller` feature and no `node` feature yet.
- The deploy builder currently runs one all-features release build:

```text
cargo build --release --all-features --manifest-path crates/lab/Cargo.toml
```

- `nodes update` currently builds one artifact and deploys that same artifact to remote nodes and the local controller.
- Artifact reuse is currently one shared mtime check against `target/release/labby`, not a per-role or per-profile cache.

## Startup Invariants

Controller startup must fail if required controller dependencies are invalid.

Node startup must not fail because controller-only dependencies are unavailable. Examples:

- missing `pnpm`
- missing Web UI source tree
- invalid upstream MCP configuration
- unavailable OAuth browser callback resources
- unavailable marketplace registry state

In node mode, those systems are not initialized.

## Rollout Verification

`nodes update` verifies success differently by role.

For node targets:

- artifact transfer succeeds
- install succeeds
- configured restart succeeds
- local node process reconnects to the controller over WebSocket
- controller reports the expected node as connected

Current implementation note:

- Remote node verification currently uses `fetch_device(...).is_ok()` as a placeholder for connected verification.
- The code does not yet have a dedicated `node_connected` controller API/helper.

For the controller target:

- artifact install succeeds
- configured restart succeeds
- controller `/health` returns success
- controller fleet WebSocket listener is available
- previously connected nodes reconnect within the rollout readiness window

Current implementation note:

- Controller rollout verification currently checks only `GET /health` on `127.0.0.1:8765`.
- It does not yet verify WebSocket listener readiness.
- It does not yet wait for expected nodes to reconnect after the controller restarts.

`systemctl is-active` is not sufficient readiness proof. A process can be active while the HTTP/WebSocket listener is unavailable or while startup is still initializing.

## Systemd Integration (Future)

The current implementation uses HTTP health probes (`/health`, `/ready`) and WebSocket
connection verification to confirm readiness after restart. `systemctl is-active` is used
only to confirm the process started (liveness), and is always followed by HTTP/WS readiness
probes — it is never the sole check.

The preferred future contract is:

- `Type=notify` in the systemd unit
- Process sends `READY=1` via `sd_notify` after HTTP/WS listeners bind and initial startup completes

This would allow systemd to gate dependent services and restart policies on true readiness.
Until implemented, systemd `is-active` reflects only process liveness, not application readiness.

## Deprecated Behavior

The old non-controller HTTP phone-home model is deprecated.

These node-local routes are not part of the node runtime contract:

- `POST /v1/nodes/status`
- `POST /v1/nodes/metadata`
- `POST /v1/nodes/oauth/relay/start`
- `GET /v1/nodes`
- `GET /v1/nodes/{node_id}`
- `GET /v1/nodes/enrollments`

They may continue to exist on the controller for operator and compatibility flows, but nodes must not require them locally.

## Security Boundaries

Controller APIs are operator-facing and protected by the configured controller auth model.

Node WebSocket sessions authenticate during initialization using node enrollment credentials pinned by the controller.

Node mode must not load controller credentials or expose controller-authorized APIs locally.

Controller-to-node command execution must remain allowlisted and auditable. Destructive operations require an explicit policy gate before they are routed to a node.
