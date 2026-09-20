---
title: "CLI Surface"
created: "2026-07-30"
updated: "2026-09-20"
---

# CLI Surface

The `labby` binary exposes a resource-oriented operator interface over the existing shared operation handlers. The executable Clap command tree is the source of truth for parsing, help, generated reference documentation, and shell completion. MCP service/action identifiers are a separate API contract and do not determine public CLI spelling.

The complete reference is [generated CLI help](../generated/cli-help.md), with a [machine-readable inventory](../generated/cli-help.json) and [command migration map](../generated/cli-migration.md).

## Grammar and discovery

Public command names are separate words without hyphens or concatenated substitutes. Flags retain normal spelling, including `--dry-run`. Resource names such as `linear-notification-worker` remain valid. The common grammar is `labby RESOURCE ACTION [NAME]`.

```bash
labby --help
labby help --all
labby help server --all
labby help --search oauth
labby help --all --json
labby server auth --help
labby completions bash
```

Help and completion work without a running gateway, a valid configuration file, or loaded credentials. Full inventory output never hides descendants behind a count. Every generated usage line contains the complete command path. Normal Clap help remains textual; use the explicit `help --json` interface for structured discovery. Repository-only `docs` and process-supervisor `internal` helpers are not advertised as public operator commands. JSON help includes the selected command's description, fully qualified usage, and option help even for a leaf with no child commands. The `commands` array contains the requested descendant inventory without duplicate or phantom paths.

## Resource groups

| Group | Responsibility |
| --- | --- |
| `context` | Save and select non-secret connection destinations in the existing host configuration. |
| `gateway` | Selected Labby daemon: status, reload, sessions, URLs, and usage. |
| `server` | Upstream MCP server configuration, testing, lifecycle, discovery/import, quarantine, and per-upstream authentication. |
| `route` | Protected MCP route configuration; `replace` is a full replacement operation. |
| `loadout` | Reusable capability selections; `set` patches only supplied fields. |
| `auth` | Operator login, offline bootstrap, owner linking, callback relays, and explicitly separate shared-provider revocation. |
| `code` | Bounded catalog search/schema inspection, source execution, settings, UI, and metadata hints. |
| `snippet` | Saved executable snippets in the local installation. |
| `skill` | Local registry reads and daemon-backed upstream trust/exposure policy. |
| `doctor` | Configuration, environment, authentication, proxy, and relay diagnosis. |
| `logs` | Bounded local rolling process-log queries; explicit deployment journal access. |
| `setup` | Guided first-run onboarding, prerequisite checking, and explicit repair. |
| `host` | Binary installation/update, native update schedule, system service, and Incus deployment. |
| `plugin` | Plugin listing, installation, synchronization, export, hooks, and connectivity checks. |
| `config` | Redacted host snapshot inspection and validation, setup state, draft discard, and direct-proxy defaults. |
| `state` | Offline access migration and durable-state export, verification, and restore. |
| `serve`, `mcp`, `proxy` | HTTP runtime, stdio MCP runtime, and explicitly selected ephemeral upstream proxy. |

Feature-gated groups reflect the compiled binary. The optional `fs` capability remains an MCP/API/web service, not a standalone CLI tree.

```bash
labby server add docs --url https://example.invalid/mcp
labby server get docs
labby server set docs --url https://new.example.invalid/mcp
labby server test docs
labby server restart docs
labby server auth login docs
labby server auth status docs
labby route list
labby loadout list
labby host service status
labby host incus backup validate --help
```

`server list` preserves the configured-upstream view. `server status` preserves the detailed runtime-state view. `gateway status` contacts the authoritative daemon and reports reachability with its upstream state; it does not claim every configured upstream is healthy merely because the daemon responded.

`server set` and `loadout set` patch supplied fields. `route replace` retains the existing complete-replacement semantics, including existing project-preservation rules. Authentication to Labby (`auth login`) is distinct from an upstream OAuth flow (`server auth login NAME`). Central Google credential revocation is under `auth provider google revoke` because it also affects dependent grants and retains explicit confirmation.

## Target and execution contracts

The grammar adapters lower into existing typed operations rather than duplicating authorization, configuration, transport, or destructive-action rules. Existing service/action identifiers and result projections remain unchanged unless a documented command-specific change says otherwise.

Daemon-backed operations support global `--server URL` or `--context NAME` selectors, which are mutually exclusive. Explicit invocation selectors win over server URL environment settings; environment settings win over the persisted default context. `--team-id` overrides the selected context's Team ID on supported authority-aware paths; it never grants access. Explicit CLI destinations use the existing destination-bound OAuth session, not an unrelated environment bearer token. No second credential store is introduced.

```bash
labby context add homelab --server https://example.invalid --use
labby context list
labby context get homelab
labby --context homelab server list
labby context set homelab --team-id personal
labby context clear
labby context remove homelab
labby auth status --server https://example.invalid
labby auth logout --server https://example.invalid
```

Contexts occupy only the `[cli]` table of the canonical host `config.toml`. Writes reuse its lock and atomic writer and preserve unrelated tables and comments. Names are bounded, URLs cannot contain credentials, query strings, or fragments, and plaintext HTTP is limited to loopback. Active contexts must be cleared or replaced before removal. Repeating the same selection is a no-op. `auth status` reports locally saved session presence without claiming online authentication; `auth logout` removes only that destination's saved session, not its provider account.

`config show` emits a redacted snapshot; `config check` validates it without writing or loading environment overrides. These and context reads work without a daemon. Explicit remote selectors on host-local operations are rejected rather than silently ignored. An implicit saved context does not retarget local host administration.

Local snippet operations and local skill reads remain local. Private/shared artifact-backed skills still require the appropriate authenticated HTTP or MCP adapter. Renaming a command does not change its caller scope.

Code Mode selects its target before executing. Once a remote execution is attempted, any failure is returned without a local replay. A transport error can mean that side effects occurred before the response was lost. Inspect state and correlated logs before deciding whether to retry. Local execution remains possible only when no daemon was selected.

```bash
labby code search "oauth" --limit 10
labby code describe example.tool
labby code run --file ./task.js
labby code run --file - < ./task.js
```

Catalog inputs are escaped as data, not interpolated as executable user JavaScript. Search result limits are bounded. Source files and stdin are read within the configured source-allocation limit. `--file -` requires redirected input and does not unexpectedly wait on an interactive terminal.

## Guided input and duration operands

In an interactive terminal, `labby server add` asks for the missing name and HTTP or stdio transport, shows the equivalent redacted command, and asks for confirmation. It fills the same typed arguments used by a complete invocation. Fully supplied commands do not open a separate wizard. `--no-input`, `--json`, or noninteractive input disable prompting and fail with actionable missing-input or confirmation errors. Cancellation never dispatches creation.

Timeout operands use explicit positive integer units: `ms`, `s`, `m`, or `h`, bounded to 24 hours unless an operation has a tighter limit. Existing whole-second APIs reject fractional seconds rather than rounding. Restart completion accepts 1ms through 5m.

```bash
labby --no-input server add docs --url https://example.invalid/mcp
labby server auth login docs --wait --timeout 2m
labby server restart docs --timeout 30s
```

`server auth login --wait` exits successfully only when authentication completes.
With `--json`, it writes one final result containing `authenticated` and
`timed_out`; a timeout returns a nonzero exit status. For URL-only waiting,
`--no-browser` prints the authorization URL to stderr immediately.

## Offline cached completion

Static shell completion remains available without configuration. Optional resource-name completion uses an explicitly refreshed local snapshot, never a network call or OAuth refresh from a Tab key.

```bash
labby --context homelab completions refresh
labby completions zsh --resources
labby --context homelab completions query -- server get doc
labby --context homelab completions clear
```

Snapshots contain safe names only and are bound to the normalized destination, Team authority, and credential fingerprint. They expire after 15 minutes, contain at most 2048 names per resource and 256 KiB total, and never contain raw credentials. Missing, malformed, oversized, expired, or differently scoped caches yield no resource names while static command suggestions remain available. A partially failed refresh replaces the affected category with no entries instead of retaining stale names; successful categories are preserved in the new snapshot, and the refresh returns a failing exit status with per-category errors. Local snippet names stay local. Clearing affects only the selected authority.

## Lifecycle completion and bulk operations

`server enable`, `server disable`, and `server restart` accept explicit names or `--all`, never an omitted name as an implicit bulk selection. The CLI takes one visible inventory from one selected gateway, preflights the entire selection, rejects duplicates or unknown names before mutation, and processes at most 128 targets sequentially. Bulk restart selects enabled servers only. A dry run may read that inventory but does not dispatch mutations.

```bash
labby server restart alpha beta --timeout 30s
labby server restart --all --dry-run
labby server restart alpha --no-wait
labby server disable alpha beta --dry-run
```

The default restart waits for the shared gateway transaction within the supplied per-server budget. A successful CLI result requires a completed transaction and a connected replacement. `--no-wait` reports acceptance, not completion. A timeout or lost response is never replayed against the same or another target; the gateway may continue its already-started operation. The shared backend retains in-flight restart deduplication. Repeated enable/disable state updates do not rewrite matching durable configuration or replace an already-matching pool; credential changes and reconciliation of divergent state are not skipped.

Lifecycle output is an aggregate result on stdout, including partial failures: selected count, failure count, destination, Team ID, request ID, and one outcome per target. The exit status is nonzero when any target fails. Each target outcome is logged at default verbosity with that request ID, elapsed time, and error classification, without raw payloads. Preflight failures still use the normal stderr error envelope. Partial completion is not an atomic rollback; inspect the individual outcomes before retrying.

## Errors, output, and logs

Finite structured operations honor `--json`. Failures that prevent a result are one JSON error envelope on stderr with no console-log prefix; result data stays on stdout. Commands reporting multiple outcomes, including lifecycle operations and completion refresh, retain their structured partial-result document on stdout and signal failure through a nonzero exit status. The stdio MCP transport reserves stdout for protocol bytes. Output-option discovery stops at `--`; a forwarded program's `--json` or `--color` argument cannot change Labby's parser diagnostics or help styling. Human errors are printed independently of tracing, so `--quiet` or `LABBY_LOG=off` cannot suppress the actual failure.

Human errors report the stable error kind, complete command path, origin, side-effect classification, and recovery guidance. Runtime failures include a request ID. The same existing error contract drives JSON output. Unknown or possible side effects are never relabeled as safely unexecuted.

Command-boundary logs record request ID, command path, elapsed time, outcome, and error/recovery classifications. They do not record raw argv, source code, parameter payloads, or raw error strings. Existing redactors handle sensitive details and bounded previews. Use `-v` for diagnostic events, `-vv` for trace detail, and `--quiet` to suppress console chatter. Explicit log filter configuration remains authoritative. An unavailable file-log sink does not panic or corrupt the structured error response.

```bash
labby logs --lines 50
labby logs --level error --query REQUEST_ID --json
labby logs --service gateway --action gateway.mcp.restart --json
labby logs journal --lines 100
labby logs journal --container labby --follow
```

The default `logs` command reads a bounded local rolling-log query and terminates. It supports severity, service, action, filename, and text/request-ID filters. It does not depend on systemd or probe other machines. `logs journal` explicitly selects the existing deployment journal backend. Streaming requires `--follow`; failures do not silently switch log sources. Journal output is raw text, so `logs journal --json` is rejected before starting any external process.

Destructive actions retain shared `ActionSpec.destructive` policy and existing explicit confirmations. Missing confirmation in a noninteractive session produces an actionable error rather than an invisible prompt. Supported dry-run previews return redacted parameters with `dry_run: true`, `executed: false`, and no operation dispatch. A preview does not imply that unsupported operations have gained dry-run support.

## Breaking migration and rollout

Old command prefixes are rejected with migration advice, not retained as executable hidden aliases. Update scripts, copied commands, installed completion files, and native update schedules before treating the migration as deployed.

The host updater still performs its existing host-binary and Incus workflow unless synchronization is explicitly disabled. Native schedule configuration is now `labby host update auto enable|disable|status`; newly generated launchd jobs invoke `labby host update --automatic`. Existing jobs created by older releases must be refreshed with `labby host update auto enable` as part of rollout. This branch does not silently change installed schedules.

The N-minus-one qualification driver selects the installed binary's grammar using a read-only help probe before performing an operation. That historical test compatibility is not a public command alias and never retries a failed mutation under another spelling.

Named contexts, guided creation, cached completion, explicit duration units, and lifecycle completion/bulk behavior are included in this migration. External scripts and already-installed native jobs still require rollout coordination; modifying repository consumers does not update another host automatically.

## Verification

`crates/labby/tests/cli_contract.rs` verifies public naming, fully qualified help for every public path, offline help with invalid configuration, canonical operands, migration errors, clean JSON, redacted previews, bounded logs, and correlation evidence. CLI unit tests preserve typed lowering and patch/replacement behavior; execution tests prove that a remote failure cannot invoke the local broker. Contract tests compare the complete JSON inventory exactly, inject invalid command names and aliases as negative controls, and verify that the controls fail for the intended reason. They snapshot actual installation files around offline discovery, preview, and refused removal, then prove confirmed removal really changes state. Mock gateway request counts assert local snippet administration performs no network requests. The plugin-validation recipe is parsed by the real CLI so stale command spellings fail the suite.

`cli_workflows` verifies context persistence, destination/Team/credential isolation, noninteractive creation, redacted config reads, and duration conversion. `cli_completion` verifies cache expiry, corruption, authority separation, partial refresh, and generated shell wrapper behavior. `cli_lifecycle` checks observed completion, per-target outcomes, mutation counts, and actual persisted log records, including numeric elapsed time and matching request IDs. Shared gateway tests exercise real manager idempotence, bounded restart dispatch, in-flight deduplication, and completion after callers stop waiting.

Regenerate artifacts through `cargo run -p labby --features all -- docs generate`, then run the matching `docs check` from the repository root. Generated references use the same command inventory as executable help.

## Supported product boundary

The CLI does not include ACP sessions, Registry browsing/installing, Marketplace product commands, Fleet enrollment, Deploy-product commands, or retired Agent Artifact Manager workspaces. Historical contracts remain in [the archive](../archive/retired-labby/). File Stash has no bespoke CLI tree; its shared actions use authenticated API/MCP adapters and its file bytes use HTTP upload/download routes.
