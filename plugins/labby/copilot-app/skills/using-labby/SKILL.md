---
name: using-labby
description: "Use when operating an installed Labby through its CLI, MCP, HTTP API, or web UI; updating it, checking health and logs, managing gateway upstreams and configuration, or exporting and restoring durable state. For live Code Mode calls use using-codemode; for saved snippets use using-snippets."
---

# Operating Labby

For a new installation or first-run onboarding, use `$install-labby`. This skill is the day-to-day operator reference once Labby is installed.
For live upstream discovery and execution, use `$using-codemode`. For running,
authoring, validating, or reviewing saved snippets, use `$using-snippets`.

`labby` is the Labby binary. Treat generated help and `docs/` as source of truth when this skill and the repo disagree.

## Quick Start

```bash
labby help                 # service/action catalog
labby doctor               # Full health/config audit
labby gateway status               # Quick availability check
labby --json doctor        # Machine-readable output
labby completions bash     # Generate shell completions
```

Use `labby`, not the old `lab` command name.

`labby help` is the human-readable service/action catalog. For shell command
grammar, use `docs/generated/cli-help.md` or `labby <command> --help`. For exact
service actions, read `docs/generated/service-catalog.md` and
`docs/generated/action-catalog.md`, call `labby help <service>`, or use service
`help`/`schema` actions through MCP/API dispatch.

## Common Top-Level Surfaces

| Command | Purpose |
|---------|---------|
| `labby mcp` | Start the MCP server over stdio |
| `labby serve` | Start the HTTP/API server |
| `labby doctor` | Audit config, auth, and runtime health |
| `labby gateway status` | Quick availability check |
| `labby setup` | First-run onboarding plus local setup check and repair flows |
| `labby gateway ...` | Manage proxied upstream MCP gateways and Code Mode |
| `labby server discover [--explain]` | Scan gateway-host MCP client configs for upstream servers |
| `labby server import [--dry-run] [-y]` | Preview or import discovered MCP servers into the gateway |
| `labby logs ...` | Read or follow Labby service logs |
| `labby host incus ...` | Manage the supported Incus gateway container |
| `labby host update ...` | Install a selected or latest Labby release |
| `labby state ...` | Export, verify, or restore complete durable installation state |
| `labby snippet ...` | Manage Code Mode snippets |
| `labby skill ...` | Inspect Agent Skills visible to the local CLI |
| `labby proxy ...` | Proxy a stdio MCP server to Streamable HTTP |
| `labby docs ...` | Generate and verify code-owned catalogs |

This is a common-workflow list, not a command inventory. Use
`docs/generated/cli-help.md` for the current top-level command inventory and
`labby <command> --help` for command-local grammar. Root `labby --help` and
`labby help` intentionally render the service/action catalog instead. Prefer
`setup` and `gateway` for operator workflows.

For command details and workflows, read:

- `references/operator-cli.md` for top-level CLI, setup, docs, doctor, logs, and gateway workflows.
- `references/gateway-operations.md` for server add/set/import/auth, route, and runtime operations.
- `references/microsandbox-upstream.md` when the selected upstream is a
  Microsandbox MCP server reached through SSH.
- `$using-codemode` for `codemode`, schemas, confirmations, limits, and error recovery.
- `references/config-reference.md` for `$LABBY_HOME/.env`, `$LABBY_HOME/config.toml`, and mutable gateway settings.
- `references/service-catalog.md` for generated catalog sources and action-dispatch discovery.

## CLI vs MCP

The MCP surface exposes one tool per runtime service with flat action strings:

```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "gateway.reload" } }
{ "action": "gateway.servers", "params": {} }
{ "action": "gateway.schema", "params": { "name": "github" } }
```

For direct MCP stdio use, run `labby mcp`. For browser/API/admin workflows, run `labby serve`.

## Code Mode and snippets

For live upstream discovery, execution, batching, and recovery, load `$using-codemode`.
For saved Code Mode workflows, load `$using-snippets`.

## Agent response notifications

The canonical `$using-codemode` skill owns receiving, acknowledgment, scope, and
producer instructions for notices attached to Code Mode responses. Authorized
producers use `POST /v1/notifications/agent`; do not edit the operator notification
feed or the SQLite file to inject messages. Inbox addresses grant no authority.
Delivery is not evidence that the underlying job completed successfully.

## Configuration

Config lives in `$LABBY_HOME/.env` and `$LABBY_HOME/config.toml` (default
`~/.labby`) using Labby's documented load order. Common env keys:

```bash
LABBY_MCP_HTTP_TOKEN=...
LABBY_GW_<NAME>_AUTH_HEADER=Bearer ...
```

Labby-owned config is operator/gateway config. Use generated env docs and
gateway service-config actions for current fields.

## Dev Commands

Inside the Labby repo, default verification is all-features:

```bash
just check
just test
just lint
just build
just run -- help
```

If you run a narrow command for speed, treat the result as provisional until the all-features path is checked.

## Troubleshooting

- Check current commands with `labby --help` or `labby <command> --help`.
- Use `labby doctor --json` when you need structured evidence.
- For MCP stdio problems, verify `labby mcp`; for HTTP/browser problems, verify `labby serve`.
- For stale docs, refresh generated docs before editing hand-written guidance.
