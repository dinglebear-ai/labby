# Gateway Operations

Use this reference for upstream MCP gateway registration, runtime state, OAuth,
import/discovery, protected routes, and Code Mode enablement.

## Gateway Model

Labby is the operator gateway. External MCP servers are registered as upstream
servers in that gateway, then exposed through policy-filtered MCP resources/tools
and the public Code Mode `codemode` tool.

Common CLI state checks:

```bash
labby server list --json
labby server get <name> --json
labby server status --json
labby gateway urls --json
```

Common action-dispatch equivalents:

```json
{ "action": "gateway.list", "params": {} }
{ "action": "gateway.get", "params": { "name": "<name>" } }
{ "action": "gateway.mcp.list", "params": {} }
{ "action": "gateway.public_urls.get", "params": {} }
```

## Adding And Testing Upstreams

The CLI tests a configured upstream server by name. Add it first (or update an
existing server), then test the saved configuration:

```bash
labby server add candidate --url https://example.invalid/mcp --json
labby server test candidate --json
labby server set candidate --url https://new.example.invalid/mcp --json
labby server test candidate --json
```

Add HTTP or stdio upstreams:

```bash
labby server add docs --url https://example.invalid/mcp --json
labby server add local-tool --command node --arg server.js --json
```

If bearer auth is needed, prefer an env-var reference:

```bash
labby server add private-tool \
  --url https://example.invalid/mcp \
  --bearer-token-env PRIVATE_TOOL_BEARER_TOKEN \
  --json
```

`bearer_token_env` must be an environment variable name, not the raw token
value. The name is operator-controlled; `PRIVATE_TOOL_BEARER_TOKEN` above is an
example, not a built-in Labby variable.

When a public HTTP/SSE upstream requires no auth, omit `bearer_token_env` and
OAuth config. Labby supports no-auth HTTP upstreams.

## Updating And Removing Upstreams

```bash
labby server set <name> --url https://new.example.invalid/mcp --json
labby server set <name> --bearer-token-env NEW_GATEWAY_BEARER_TOKEN --json
labby server remove <name> --json
labby gateway reload --json
```

Use `gateway.reload` after direct config/env changes to reconcile runtime state.
Only reload promises to pick up changed bearer-token env values.

## Discovery And Import

Discovery scans local MCP client configs from known editors/tools:

```bash
labby server discover --json
labby server discover --clients claude-code,codex --json
labby server discover --explain --json
```

`--explain` returns the selected client kinds, matching config paths,
per-client discovery counts, and the number of duplicate names omitted. It
never returns environment values or raw config contents. Discovery runs on the
selected gateway host.

Import is destructive because it mutates gateway config:

```bash
labby server import --name <server> -y --json
labby server import --all -y --json
labby server import --all --dry-run --json
```

`--dry-run` returns the redacted import plan and any skip reasons without
writing gateway configuration or reconciling runtime state.

Pending discovered servers can be reviewed and approved/rejected:

```bash
labby server pending list --json
labby server pending approve <name> -y --json
labby server pending reject <name> -y --json
```

Use `--dry-run` on pending approve/reject when available.

## Runtime MCP Lifecycle

Use `server` for runtime lifecycle and process cleanup:

```bash
labby server status --json
labby server enable <name> --json
labby server restart <name> --json
labby server disable <name> --cleanup --json
labby server cleanup <name> --dry-run --json
labby server cleanup <name> --aggressive --json
```

The runtime list includes discovery counts and likely stale process counts. Use
cleanup when the config changed but old server processes remain.

## Upstream OAuth

OAuth is per upstream and subject. Shared gateway credential flows are available
from CLI:

```bash
labby server auth status <name> --json
labby server auth login <name> --no-browser --json
labby server auth login <name> --wait --json
labby server auth logout <name> --json
```

Use the server-side OAuth status path when browser OAuth looks connected but
runtime calls still fail. Code Mode admin/trusted paths use the shared gateway
subject; non-admin scoped users may use their own subject.

## Code Mode Surface

The gateway-wide code-mode setting exposes the synthetic public MCP tools
`codemode` instead of raw upstream tools:

```bash
labby code status --json
labby code enable --json
labby code disable --json
```

In action dispatch:

```json
{ "action": "gateway.code_mode.get", "params": {} }
{ "action": "gateway.code_mode.set", "params": { "enabled": true } }
```

## Gateway Schema Resources

For complete connected-upstream inspection:

```json
{ "action": "gateway.servers", "params": {} }
{ "action": "gateway.schema", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_tools", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_resources", "params": { "name": "<upstream>" } }
{ "action": "gateway.discovered_prompts", "params": { "name": "<upstream>" } }
```

MCP resources:

- `lab://gateway/servers`
- `lab://gateway/<name>/schema`

Resources are cache-backed and exposure-policy filtered. If a tool is absent,
check exposure policy and reload runtime state before concluding the upstream
does not provide it.

## Protected Routes

Protected routes publish Lab-managed public MCP routes with OAuth protection:

```bash
labby route list --json
labby route test route \
  --public-host lab.example.invalid \
  --public-path /mcp \
  --upstream upstream-name \
  --json
labby route add route \
  --public-host lab.example.invalid \
  --public-path /mcp \
  --upstream upstream-name \
  --scope lab:read \
  --json
```

Protected routes may use either an `upstream` or a `backend_url`, not both.
Backend targets are validated to avoid unsafe local/link-local targets.

## Config Mutation Actions

Use `labby server add`, `labby server set`, `labby server remove`, and
`labby server import` for upstream configuration. Use `labby gateway reload`
to reconcile configuration with the daemon runtime. Discover the live action
schema before dispatching the equivalent MCP action. Values are redacted on
reads when fields are marked secret.

## Common Failure Routing

| Symptom | First check |
| --- | --- |
| Upstream missing from Code Mode search | `labby server status --json`, then `gateway.schema` |
| OAuth works in browser but runtime fails | `labby server auth status <name> --json` |
| Tool absent from one upstream | `gateway.discovered_tools`, exposure policy, reload |
| Stale process or old schema | `labby server cleanup <name> --dry-run --json` |
| Config changed but runtime did not | `labby gateway reload --json` |
| Import keeps reappearing | pending/tombstone actions in generated action catalog |
