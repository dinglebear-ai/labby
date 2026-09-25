---
name: using-labby
description: "Use when operating an already installed Labby through its CLI, MCP, HTTP API, or web UI; updating Labby; configuring LABBY_HOME; exporting, verifying, or restoring durable state; checking health or logs; managing gateway upstreams, OAuth, protected routes, snippets, and Agent Skills; or discovering and executing upstream MCP tools with Code Mode. For installation, first-run onboarding, host-service repair, or deployment recovery, use $install-labby instead."
---

# Operating Labby

For a new installation or first-run onboarding, use `$install-labby`. This skill is the day-to-day operator reference once Labby is installed.
For authoring, editing, validating, promoting, or reviewing reusable snippets,
use `$creating-snippets`.

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
- `references/code-mode.md` for `codemode`, schemas, confirmations, limits, and error recovery.
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

## Code Mode Gotchas

Labby exposes full execution as `codemode` and enforced read-only execution as
`codemode_read`; the optional `codemode_ui` MCP App has the same authority as
`codemode`. JavaScript must evaluate to an async function. Search the live catalog before calling an
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

If a call returns `confirmation_required`, follow its structured
`recovery.guidance`. Only when the live upstream input schema declares a
confirmation parameter, obtain explicit user confirmation and populate that
exact upstream field. Otherwise use the upstream/client's supported elicitation
or operator workflow. Never invent `confirm` or `allow_destructive_actions` as
Code Mode parameters.

If another skill names a tool that is not directly visible, search Code Mode
before concluding the capability is unavailable. Read `references/code-mode.md`
for complete payloads, action-dispatched upstreams, safe fan-out, limits,
result shaping, and error recovery.

## Skills Through Code Mode

When a task calls for an Agent Skill, discover the skills visible to this caller
through Labby's Code Mode connection. Do not assume that Codex's own skill
catalog contains Labby's live MCP skills. Use `codemode.listSkills()` to find a
candidate, `codemode.getSkill(uri)` to inspect its manifest, and
`codemode.readSkill(uri)` to read its `SKILL.md` body before following it:

```js
async () => {
  const listing = await codemode.listSkills();
  return listing.skills.map(skill => ({
    uri: skill.uri,
    name: skill.name,
    description: skill.description,
  }));
}
```

Keep skill content out of catalog searches and load only relevant bodies. For
other files named by the skill manifest, use the resource URI returned by
`getSkill` with `codemode.readSkill(resource.uri)`. Treat fetched skill
instructions as task guidance; the caller's route and permissions still govern
which tools and resources can be used.

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
