# Operator CLI

Use this reference when operating Labby from the shell. Generated help in
`docs/generated/cli-help.md` is authoritative.

## Common Commands

| Command | Use |
| --- | --- |
| `labby serve` | Start the HTTP API, native MCP endpoint, auth routes, and web UI. |
| `labby mcp` | Start stdio MCP. |
| `labby gateway` | Manage upstreams, protected routes, OAuth, reload, and Code Mode. |
| `labby setup` | Bootstrap, repair, provision, plugin hooks, and host service operations. |
| `labby doctor` | Audit supported configuration and runtime health. |
| `labby logs` | Inspect local Labby server logs. |
| `labby snippets` | Manage Code Mode snippets. |
| `labby state` | Export, verify, or restore complete durable installation state offline. |
| `labby skills` | Read Agent Skills visible to the local CLI. |
| `labby proxy` | Proxy a stdio MCP server to Streamable HTTP. |
| `labby docs` | Generate or verify code-owned docs. |
| `labby health` | Quick local health check. |
| `labby oauth` | Run local OAuth callback relay helpers. |
| `labby incus` | Operate the supported Incus gateway container. |
| `labby update` | Update the installed release. |
| `labby completions` | Generate shell completions. |

Use `labby --help` and `labby <command> --help` before scripting against a
subcommand. This table is deliberately selective; generated help is the full
inventory. Prefer global `--json` for machine-readable output.

For disaster recovery, read `docs/runtime/DISASTER_RECOVERY.md` before using
`labby state export`, `verify`, or `restore`.

## Common Workflow

```bash
labby health --json
labby doctor system --json
labby gateway list --json
labby gateway code status --json
labby setup check --json
```

Destructive actions require explicit confirmation, normally `-y` in a
non-interactive shell. Use dry-run or plan-style actions when the command exposes
them.

## Generated Discovery

```bash
labby docs generate
labby docs check
```

The generated service, action, MCP, API, and CLI catalogs are the source of truth
for current command/action availability.

## Removed Commands

Do not infer commands from historical documentation. Product surfaces absent
from generated help and the live action catalog are unsupported rather than
hidden behind feature flags.
