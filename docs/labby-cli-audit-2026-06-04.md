# Labby CLI Audit — 2026-06-04

Tested installed binary: **labby 0.21.4** at `/home/jmagar/.local/bin/labby`  
Source version: **0.22.1** (does not currently build — see Compile Errors)

---

## Summary

| Category | Count | Severity |
|----------|-------|----------|
| Source compile errors | 3 | Critical (blocks build) |
| Runtime UX issues | 4 | Minor |
| Empty `--help` descriptions | 20+ | Cosmetic |

---

## Commands Tested

### Working Correctly

| Command | Verdict |
|---------|---------|
| `labby health` | ✅ PASS — lists services, JSON flag works |
| `labby --json health` | ✅ PASS — valid JSON array |
| `labby gateway list` | ✅ PASS — 30 servers shown (25 connected) |
| `labby gateway get <name>` | ✅ PASS — detailed config + runtime state |
| `labby gateway reload` | ✅ PASS — returns changed flags |
| `labby gateway discover` | ✅ PASS — scans all editors |
| `labby gateway public-urls` | ✅ PASS — shows configured URLs |
| `labby gateway mcp list` | ✅ PASS — full MCP runtime state |
| `labby gateway mcp auth start/status/clear --help` | ✅ PASS — all subcommands available |
| `labby gateway tool-search status/enable/disable --help` | ✅ PASS |
| `labby gateway pending list/approve/reject --help` | ✅ PASS |
| `labby gateway code exec --help` | ✅ PASS |
| `labby doctor` (no subcommand) | ✅ PASS — runs all checks |
| `labby doctor system` | ✅ PASS — config/docker/disk checks |
| `labby doctor auth` | ✅ PASS — OAuth config checks |
| `labby setup check` | ✅ PASS — read-only prerequisite check |
| `labby setup repair` | ✅ PASS — idempotent repair |
| `labby setup installed-plugins` | ✅ PASS — lists all plugins |
| `labby setup services-status` | ✅ PASS — config/plugin/draft status |
| `labby setup plugin-connectivity` | ✅ PASS — lab server reachability |
| `labby setup plugin-export` | ✅ PASS — env field dump with masking |
| `labby logs local stats` | ✅ PASS — retention and drop counters |
| `labby logs local tail` | ✅ PASS — options parse correctly |
| `labby logs local stream` | ✅ PASS — correct error: use HTTP SSE |
| `labby marketplace` | ✅ PASS — action catalog displayed |
| `labby marketplace sources.list` | ✅ PASS — all 6 sources listed |
| `labby marketplace plugins.list` | ✅ PASS — full plugin list |
| `labby marketplace plugin.get --params '{"id":"..."}'` | ✅ PASS |
| `labby stash help` | ✅ PASS — 18 actions shown |
| `labby stash components.list` | ✅ PASS — 18 components listed |
| `labby stash providers.list` | ✅ PASS — empty result (∅) |
| `labby stash targets.list` | ✅ PASS — empty result (∅) |
| `labby deploy config-list` | ✅ PASS — hosts and defaults |
| `labby deploy --help` | ✅ PASS |
| `labby nodes list` | ✅ PASS — shows 1 node (controller) |
| `labby nodes enrollments list/approve/deny --help` | ✅ PASS |
| `labby completions bash` | ✅ PASS — 6907 lines of completions |
| `labby help gateway` | ✅ PASS — falls through to Clap help |

---

## Issues Found

### CRITICAL — Source Build Errors

The installed 0.21.4 binary is **not affected** by these errors. The 0.22.1 source does not compile.

**Error 1: Rate-limit methods changed signature but callers not updated**

File: `crates/lab-auth/src/state.rs`  
`check_authorize_rate_limit` and `check_register_rate_limit` were updated to accept `ip: IpAddr` for per-IP rate limiting (per lab-77y5.9/10).

File: `crates/lab-auth/src/authorize.rs` — callers not updated:
- Line 68: `state.check_authorize_rate_limit()?` → needs `(ip)?`
- Line 111: `state.check_register_rate_limit()?` → needs `(ip)?`
- Line 154: `state.check_authorize_rate_limit()?` → needs `(ip)?`

Fix: Add `ConnectInfo<SocketAddr>` extractor to `browser_login`, `register_client`, and `authorize` in `authorize.rs`; extract `.ip()` and pass through. Also update the three wrapper functions in `router.rs` (`auth_browser_login:252`, `auth_register:234`, `auth_authorize:241`).

---

### MINOR — Runtime UX Issues (affect installed 0.21.4 binary)

**Issue 1: `labby logs local search` — no positional query argument**

```
labby logs local search "error"
# error: unexpected argument 'error' found
```

Users expect `search <query>` by analogy with `labby logs search <device> <query>`.  
Must instead use: `labby logs local search --text "error"`  

The `--text` flag also has an empty description in `--help`. Inconsistency with the parent `logs search` subcommand which does accept positional args.

Files to fix: `crates/lab/src/cli/logs.rs` (add `<QUERY>` positional arg that maps to `--text`), and add help text to `--text`.

**Issue 2: `labby doctor proxy` — requires all 3 flags; doesn't default from env**

```
labby doctor proxy
# error: missing --app-url, --mcp-url, --route
```

`labby gateway public-urls` shows `LAB_PUBLIC_URL` and `LAB_MCP_GATEWAY_URL` are already configured. `doctor proxy` should default `--app-url` and `--mcp-url` from these env vars (same ones used by `gateway public-urls`), requiring only `--route` when those aren't overridden.

File: `crates/lab/src/cli/doctor.rs` — make `--app-url` and `--mcp-url` optional with env-var fallback.

**Issue 3: `labby deploy plan` — requires positional `<TARGETS>` with no default**

```
labby deploy plan
# error: the following required arguments were not provided: <TARGETS>...
```

`labby deploy config-list` already shows all configured hosts. A `--all` flag or defaulting to all configured hosts would improve usability. Minor because `deploy plan node-a` works as expected.

**Issue 4: Missing `--help` descriptions (cosmetic)**

Multiple flags and positional args have no description text in `--help`, making the CLI harder to use:

| Command | Flag/Arg | Missing |
|---------|----------|---------|
| `logs local search` | `--text` | full description |
| `logs local search` | `--after-ts`, `--before-ts`, `--limit`, `--action`, `--request-id`, `--session-id`, `--correlation-id` | descriptions |
| `logs local tail` | `--after-ts`, `--since-event-id`, `--limit` | descriptions |
| `logs search` | `<DEVICE>`, `<QUERY>` | descriptions |
| `gateway add` | `--name`, `--url`, `--command`, `--arg`, `--bearer-token-env` | descriptions |
| `gateway update` | `--new-name`, `--url`, `--command`, `--arg`, `--bearer-token-env` | descriptions |
| `gateway test` | `--name` | description |
| `nodes get` | `<NODE_ID>` | description |

---

## Commands NOT Tested (destructive or require preconditions)

- `gateway add/update/remove` — mutates gateway config
- `gateway import` — imports discovered servers
- `deploy run/rollback` — deploys to SSH targets
- `nodes update` — rolls out binary to nodes
- `gateway mcp auth start/open` — OAuth flow requires browser
- `stash component.import/export/deploy` — filesystem mutations

---

## Environment

```
labby 0.21.4 (installed at /home/jmagar/.local/bin/labby)
source v0.22.1 at /home/jmagar/workspace/lab (does not build)
platform: linux, node-a (dev box)
```
