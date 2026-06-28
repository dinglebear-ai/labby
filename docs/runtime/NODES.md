# Nodes

`nodes` is the canonical operator surface for Lab.

## Vocabulary

- `node`: one managed runtime
- `nodes`: the operator control surface
- `controller`: the coordinating node for the runtime
- `node_id`: the only public runtime identifier

## Public Surface

Canonical CLI:

```bash
labby nodes list
labby nodes get node-b
labby nodes enrollments list
```

Canonical HTTP routes:

- `GET /v1/nodes`
- `GET /v1/nodes/{node_id}`
- `GET /v1/nodes/enrollments`
- `POST /v1/nodes/enrollments/{node_id}/approve`
- `POST /v1/nodes/enrollments/{node_id}/deny`
- `POST /v1/nodes/logs/search`
- `POST /v1/nodes/hello`
- `POST /v1/nodes/oauth/relay/start`
- `POST /v1/nodes/syslog/batch`
- `GET /v1/nodes/ws`

Canonical websocket contract (see `docs/FLEET_METHODS.md` for full spec):

- Auth model: the WS endpoint is outside bearer-auth middleware; `initialize` establishes the session
- `initialize` — enrollment validation; 10-second deadline from first connection
- `nodes/ping` — liveness check
- `nodes/status.push` — telemetry push
- `nodes/metadata.push` — discovered config push
- `nodes/log.event` — structured log batch push
- `nodes/device.enroll` — node identity upsert (idempotent, role-checked)
- `nodes/command.invoke` — initiate remote command execution
- `nodes/command.output` — stream command output chunks
- `nodes/command.result` — report final exit status
- `nodes/peer.invoke` — peer RPC (not yet implemented)
- MCP demux allowlist: `lab://catalog` plus service `help`/`schema` actions

Canonical config:

```toml
[node]
controller = "controller"
```

## Serialization Boundary

Per `docs/design/SERIALIZATION.md`:

- `lab-apis` owns SDK wire models
- `lab` owns product-surface routes, payloads, envelopes, and presentation

Fresh product output uses `node_id` only.
