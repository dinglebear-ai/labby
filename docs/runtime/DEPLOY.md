# Deploy

This document covers the node-runtime deployment model introduced by `labby serve`.

## Topology

One node is the configured `controller`. Every other node points at it with:

```toml
[node]
controller = "controller"
```

The controller hosts:

- `/mcp`
- `/v1/{service}`
- `/v1/gateway`
- Web UI
- `/v1/nodes/ws`
- `/v1/nodes/*` fleet ingest and query routes

Non-controller nodes host:

- `/health`
- `/ready`
- `/v1/nodes/*`

They do not expose the operator control plane.

## Controller Setup

1. Choose a stable hostname for the controller host.
2. Set `[node].controller` to that hostname in `config.toml` on every non-controller machine.
   Legacy `[device].master` is still read for compatibility, but new config should use `[node].controller`.
3. Start `labby serve` on the controller host.
4. If the controller binds beyond loopback, configure auth before startup.

Example:

```bash
LAB_MCP_TRANSPORT=http
LAB_MCP_HTTP_HOST=0.0.0.0
LAB_MCP_HTTP_PORT=8765
LAB_MCP_HTTP_TOKEN=replace-me
labby serve
```

## Node Setup

Each non-controller node needs:

- `[node].controller` set to the controller hostname
- HTTP reachability to `http://<controller>:<port>`
- WebSocket reachability to `ws://<controller>:<port>/v1/nodes/ws` or `wss://...`

Example `config.toml`:

```toml
[node]
controller = "controller"

[mcp]
port = 8765
```

Then start:

```bash
labby serve
```

The node runtime will queue metadata and bootstrap logs locally, open its WebSocket session automatically, and keep retrying until the controller admits the node.

## Enrollment Rollout

New or rotated nodes are not admitted automatically.

Expected rollout:

1. deploy the controller first
2. start or restart non-controller nodes
3. list pending enrollments on the controller
4. approve or deny nodes explicitly

Examples:

```bash
labby nodes enrollments list
labby nodes enrollments approve node-a
labby nodes enrollments deny workstation-wsl --reason "unexpected token"
```

## Operational Notes

- if `[node].controller` (and the legacy `[device].master`) is omitted, the local machine resolves as the controller
- fleet node identity is pinned by the enrollment store, not by the shared bearer token
- node inventory and logs are stored in memory on the controller in this implementation
- enrollment records survive controller restarts
- Web UI requests to a non-controller node return `403` with a clear disabled message

## Verification

Useful checks after deployment:

```bash
curl http://<node>:8765/health
curl -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" http://<controller>:8765/v1/nodes
curl -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" http://<controller>:8765/v1/nodes/enrollments
labby nodes list
labby nodes enrollments list
labby logs search <node_id> <query>
```

## Readiness Verification Model

`nodes update` confirms readiness using HTTP health probes and WebSocket connection
verification, not `systemctl is-active` alone.

For remote node targets:

1. Binary transfer and install succeed.
2. `systemctl restart` (or wrapper command) is invoked.
3. `<binary> --version` is run on the remote host to confirm the new binary is executable.
4. The controller polls `node_connected(<node_id>)` with exponential backoff (2s, 4s, 8s, capped at 16s)
   until the node reconnects over WebSocket or the rollout timeout expires.

For the local controller target:

1. Binary install succeeds (previous binary is backed up with a timestamp extension).
2. `systemctl restart` is invoked; `systemctl is-active --wait` confirms process liveness.
3. `GET /health` and `GET /ready` are probed on `127.0.0.1:<port>` to confirm application readiness.

`systemctl is-active` is a liveness check only. A process can be active while the HTTP/WebSocket
listener is still initializing. The HTTP probes are the authoritative readiness gate.

## Controller Self-Update Recovery

When `nodes update` updates the local controller and readiness verification fails after install:

1. The previous binary is backed up before install. The backup path is included in the result
   JSON under `backup_path` (omitted when no previous binary existed).
2. A `recovery_hint` field in the result JSON provides the exact restore command.
3. To restore manually:
   ```
   sudo install -m 755 <backup_path> /usr/local/bin/labby
   sudo systemctl restart lab
   ```
4. Verify recovery:
   ```
   curl -sf http://127.0.0.1:<port>/health
   curl -sf http://127.0.0.1:<port>/ready
   ```

The backup extension uses a Unix timestamp, e.g. `lab.bak.1714000000`. Fresh installs (no prior
binary) produce no backup and omit the `backup_path` field from the result.
