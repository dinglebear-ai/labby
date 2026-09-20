# Operator CLI

Use the executable command tree, not MCP service/action identifiers, when operating Labby from a shell. Public command names have no hyphens. Flags and resource names may contain hyphens.

## Discover Before Executing

```bash
labby --help
labby help --all
labby help server --all
labby help --search oauth
labby help --all --json
```

Help is offline. Generated `docs/generated/cli-help.md` and `cli-help.json` derive from the same parser and contain the complete public inventory. Retired paths fail with migration guidance; they are not hidden executable aliases.

## Resource-Oriented Workflows

```bash
labby gateway status --json
labby server list --json
labby server get axon
labby server test axon
labby server restart axon --timeout 30s
labby server auth login axon --wait --timeout 2m
labby route list
labby loadout list
labby code search "oauth" --limit 10
labby code describe example.tool
labby code run --file ./task.js
labby snippet list
labby host service status
labby plugin list
labby config show --json
labby config check
```

`gateway` describes the selected daemon; `server` manages upstream MCP servers. `set` patches supplied fields, while `route replace` replaces configuration. `host` contains local installation, update, service, and Incus operations. `setup` is onboarding/check/repair, not ongoing server administration. `auth relay` contains callback-relay operations. `serve`, `mcp`, and `proxy` are distinct runtime/transport entry points.

## Select an Authority Explicitly

```bash
labby context add homelab --server https://example.invalid --use
labby --context homelab server list
labby auth status --context homelab
labby context clear
```

Explicit `--server` or `--context` wins over environment targets; environment targets win over the saved default. An explicit context or server URL uses the existing destination-bound OAuth session, never an unrelated environment bearer token. Team selectors do not grant access. Host-local commands reject explicit remote selectors. Local snippet and skill operations stay local.

## Automation and Safety

Use `--json` and `--no-input` in scripts. Missing arguments or confirmation then fail rather than prompting. `server add` guides missing input only in an interactive terminal, displays the equivalent redacted command, and requires confirmation before dispatch.

Lifecycle operations accept explicit names or `--all`; omission never means all servers. Supported previews use `--dry-run`. The default restart checks completion and replacement connection health. `--no-wait` reports acceptance only. Partial results stay on stdout with per-target outcomes and a request ID; any target failure produces a nonzero exit status. Do not replay uncertain operations automatically.

```bash
labby server restart alpha beta --dry-run
labby logs --query REQUEST_ID --json
labby --context homelab completions refresh
labby completions zsh --resources
```

Completion refresh is explicit. Tab reads a bounded local snapshot keyed by destination, Team, and credentials. Expired, corrupt, or differently scoped snapshots provide no resource names and never trigger network access.

Destructive operations require the confirmation flag their own help specifies, normally `--yes`. Before `state export`, `state verify`, or `state restore`, read `docs/runtime/DISASTER_RECOVERY.md`. These are offline installation-state operations.

## Maintainer Verification

Repository tooling uses `just docs-generate` and `just docs-check`. Developer-only documentation helpers are intentionally not advertised as operator commands. The behavioral contract and rollout requirements are in `docs/surfaces/CLI.md`.
