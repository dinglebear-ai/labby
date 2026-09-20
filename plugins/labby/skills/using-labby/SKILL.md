---
name: using-labby
description: "Use when operating an already installed Labby through its CLI, MCP, HTTP API, or web UI; updating Labby; configuring LABBY_HOME; repairing or rolling back an Incus gateway or host service; exporting, verifying, or restoring durable state; checking health or logs; managing gateway upstreams, OAuth, or protected routes; or discovering and executing upstream MCP tools with Code Mode. For a new installation or first-run onboarding, use $install-labby instead."
---

# Using the `labby` CLI

For a new installation or first-run onboarding, use `$install-labby`. This skill is the day-to-day operator reference once Labby is installed.

`labby` is the Labby binary. Treat generated help and `docs/` as source of truth when this skill and the repo disagree.

## Quick Start

```bash
labby help                 # CLI command help
labby doctor               # Full health/config audit
labby gateway status               # Quick availability check
labby --json doctor        # Machine-readable output
labby completions bash     # Generate shell completions
```

Use `labby`, not the old `lab` command name.

`labby help` is Clap command help in the current CLI. For service/action catalogs,
read `docs/generated/service-catalog.md`, `docs/generated/action-catalog.md`, or
use service `help`/`schema` actions through MCP/API dispatch.

## Common Top-Level Surfaces

| Command | Purpose |
|---------|---------|
| `labby mcp` | Start the MCP server over stdio |
| `labby serve` | Start the HTTP/API server |
| `labby doctor` | Audit config, auth, and runtime health |
| `labby gateway status` | Quick availability check |
| `labby setup` | First-run/setup and plugin install flows |
| `labby gateway ...` | Manage proxied upstream MCP gateways and Code Mode |
| `labby server discover` | Scan local MCP client configs for upstream servers |
| `labby server import [-y]` | Import discovered MCP servers into the gateway |
| `labby logs ...` | Read or follow Labby service logs |
| `labby host incus ...` | Manage the supported Incus gateway container |
| `labby host update ...` | Install a selected or latest Labby release |
| `labby state ...` | Export, verify, or restore complete durable installation state |
| `labby snippet ...` | Manage Code Mode snippets |
| `labby skill ...` | Inspect Agent Skills visible to the local CLI |
| `labby proxy ...` | Proxy a stdio MCP server to Streamable HTTP |
| `labby docs ...` | Generate and verify code-owned catalogs |

This is a common-workflow list, not a command inventory. Use only current
top-level commands from `labby --help`. Prefer `setup` and `gateway` for
operator workflows.

For command details and workflows, read:

- `references/operator-cli.md` for top-level CLI, setup, docs, doctor, logs, and gateway workflows.
- `references/gateway-operations.md` for gateway add/update/import/OAuth/protected routes/runtime operations.
- `references/code-mode.md` for `codemode`, schemas, confirmations, limits, and error recovery.
- `references/config-reference.md` for `~/.labby/.env`, `config.toml`, and mutable gateway settings.
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

## Code Mode Gotchas

Labby exposes the public Code Mode tool as `codemode`. Its JavaScript must
evaluate to an async function. Search the live catalog before calling an
upstream; do not guess tool IDs, helper names, schemas, or parameter envelopes:

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 5 });
  return hits.results.map(t => ({ id: t.id, signature: t.signature }));
}
```

Use `callTool("<upstream>::<tool>", params)` for dynamic targets. Use generated
`codemode.<upstream>.<tool>(params)` helpers only after search confirms the
path. Narrow execution with the top-level `upstreams` or `tools` allowlists.

If a call returns `confirmation_required`, inspect the live upstream schema and
put confirmation exactly where that schema requires it. Do not use
`allow_destructive_actions`; it is not a public `codemode` parameter.

If another skill names a tool that is not directly visible, search Code Mode
before concluding the capability is unavailable. Read `references/code-mode.md`
for complete payloads, action-dispatched upstreams, safe fan-out, limits,
result shaping, and error recovery.

## Configuration

Config lives in `~/.labby/.env` and `config.toml` using Labby's documented load order. Common env keys:

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
