# Operator CLI

Use this reference when operating Labby from the shell. Generated help in
`docs/generated/cli-help.md` is authoritative.

## Current Commands

| Command | Use |
| --- | --- |
| `labby serve` | Start the HTTP API, native MCP endpoint, auth routes, and web UI. |
| `labby mcp` | Start stdio MCP. |
| `labby gateway` | Manage upstreams, protected routes, OAuth, reload, and Code Mode. |
| `labby setup` | Bootstrap, repair, provision, plugin hooks, and host service operations. |
| `labby doctor` | Audit supported configuration and runtime health. |
| `labby server-logs` | Inspect local Labby server logs. |
| `labby snippets` | Manage Code Mode snippets. |
| `labby docs` | Generate or verify code-owned docs. |
| `labby health` | Quick local health check. |
| `labby oauth` | Run local OAuth callback relay helpers. |
| `labby incus` | Operate the supported Incus gateway container. |
| `labby update` | Update the installed release. |
| `labby completions` | Generate shell completions. |

Use `labby --help` and `labby <command> --help` before scripting against a
subcommand. Prefer global `--json` for machine-readable output.

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

Do not invoke ACP, Marketplace, Registry-browser, Fleet/node, Deploy-product, or
Stash commands. Those product surfaces are retired rather than hidden behind
feature flags.
