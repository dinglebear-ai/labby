# Configuration Reference

Config lives in `~/.labby/.env`. Loaded at startup by `crates/labby/src/config.rs`.

Runtime gateway settings live in `config.toml`; verify exact fields against
`crates/labby/src/config.rs` before editing.

## Env Var Naming Convention

```
LABBY_MCP_HTTP_TOKEN            # static bearer token for Labby HTTP/MCP
LABBY_GW_<NAME>_AUTH_HEADER     # auth header for one gateway upstream
```

Use `docs/generated/env-reference.md` for current Labby-owned env vars. For
gateway auth, prefer `gateway.add`/`gateway.update` with `bearer_token_env` and
let Labby derive `LABBY_GW_<NAME>_AUTH_HEADER` when possible.

## Logging

```
LABBY_LOG=labby=info,labby_apis=warn  # tracing filter directive (default)
LABBY_LOG_FORMAT=json                # emit newline-delimited JSON (for prod/CI)
```

## Code Mode Config

Root `[code_mode]` controls Code Mode limits:

```toml
[code_mode]
enabled = true
trace_params = true
result_shape_policy = "off"      # off | truncate
timeout_ms = 30000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

`gateway.code_mode.set` accepts these public fields. `result_shape_policy =
"truncate"` shapes only successful completed final `result` values for
model-facing output. It does not affect sandbox-visible `callTool()` results,
does not retain raw results, and is not redaction.

## Config Mutation

Use setup and gateway actions instead of direct `.env` edits when possible:

```json
{ "action": "gateway.service_config.get", "params": { "service": "deploy" } }
{ "action": "gateway.service_config.set", "params": { "service": "deploy", "values": {} } }
```

For upstream MCP servers, use `labby gateway add`, `labby gateway update`,
`labby gateway discover`, `labby gateway import`, and `labby gateway reload`.
For operational gateway examples, read `gateway-operations.md`.
