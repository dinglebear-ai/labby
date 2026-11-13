# Labby LiveView Console

Read-only console integration for the refined Phoenix prototype. OTP application:
`labby_console`; modules: `LabbyConsole` and `LabbyConsoleWeb`.

The console has eleven authenticated read-model screens, public readiness, and
bearer-session sign-in/sign-out. It runs beside the Rust gateway as a separate
BEAM application; it does not replace the embedded Next.js UI or desktop shell.
See [implementation status](docs/implementation-status.md) for the mock comparison
and remaining scope. This is a partial migration, not deployment qualification.

## Development

```sh
cd apps/liveview
mix setup
LABBY_CONSOLE_GATEWAY_URL=http://127.0.0.1:8080 mix phx.server
```

The console binds loopback on port 4100 by default. Set `PORT` to override it.
The gateway URL above is an example, not a discovered service or implicit default.
The initial LiveView probes the configured gateway’s public `/ready` endpoint asynchronously and offers an explicit retry. Ready, degraded, not-ready and unavailable states remain distinct. The transport also supports `/health` as a separate liveness probe.

## Integration boundaries

- Rust dispatch/runtime remains authoritative for validation, authorization,
  credentials, lifecycle, destructive actions and retry policy.
- Phoenix owns presentation and per-user UI state. It must not copy the
  prototype's session-local domain simulations into production code.
- Use an explicitly configured gateway. Never fall back to local state when a
  remote target fails. Never share a privileged gateway token across users.
- Before adding data routes, implement and test per-caller authentication,
  session/CSRF propagation, structured errors, timeouts and bounded responses.
- Consume canonical Aurora primitives/tokens through a supported Phoenix adapter;
  the generator landing page is temporary, not a new design system.

## Migration sequence

1. Runtime foundation and public readiness slice (implemented).
2. Per-user bearer-session authentication and protected transport (implemented).
3. Shared Aurora shell and eleven real read-model screens (implemented).
4. Remaining mock workspaces and mutating workflows (pending).
5. Packaging and desktop routing cutover after production qualification.

`apps/web` remains the embedded static UI. This app is a separate BEAM runtime:
LiveView HTML cannot be embedded as a static export in the Rust binary. A future
cutover needs deployment/supervision and reverse-proxy/WebSocket routing, not just
an assets-directory switch. The external prototype remains the visual reference.

## Checks

```sh
mix precommit
mix assets.deploy
```

## Public probe transport

`LabbyConsole.Gateway` accepts an explicit HTTP(S) origin only; credentials,
query strings, fragments and path prefixes are rejected. Requests have two-second
connect/read timeouts, a 64 KiB response cap, no redirects and no automatic
retries. Errors return bounded public states rather than backend bodies. No
privileged token is required or sent for these public endpoints.

Tests use a real loopback Bandit fixture and LiveView’s async test interface.
The public probe runs only after the LiveView connects. Protected pages validate
their caller during HTTP and WebSocket mounts, then load their read models
asynchronously. Gateway inventory and the additional slices are described below.

## Shell assets

The initial real shell consumes `priv/static/vendor/aurora.css` copied without
local token edits from Aurora's published `public/aurora.css`, revision
`58f21b6d06b30af2af8565223a7a234387681ba7`. Inter and Manrope font files come from
that same source. Product layout lives in `priv/static/assets/css/app.css`.
Refresh the vendor stylesheet/fonts from Aurora together; do not fork its tokens.
Only implemented routes appear in primary navigation. The authenticated shell is
available; the full prototype migration remains incomplete.

## Bearer-session authentication slice

`/sign-in` exchanges a user-supplied bearer credential at the configured gateway's
`POST /auth/bearer-session`, then validates `GET /auth/session`. The credential is
not retained. An in-memory, bounded console session vault holds the gateway cookie;
the browser receives only a random console session ID in Phoenix's signed,
HttpOnly session cookie. Gateway sessions are revalidated on each protected page
request. Sessions expire, are pruned, and disappear on console restart. Deploy a
single console instance for this initial slice; clustered session storage and
OAuth sign-in are not implemented.

`POST /sign-out` clears local access even if gateway revocation fails, and reports
unconfirmed remote revocation. Phoenix CSRF protection covers sign-in/sign-out;
gateway logout uses the gateway session's CSRF token. Credentials are filtered from
Phoenix parameter logs. Credential exchange requires HTTPS or a literal loopback
host. Terminate console HTTPS before exposing this beyond loopback.

### Test commands

From the repository root:

```sh
just liveview-test              # unit, HTTP contract, and LiveView regressions
just liveview-e2e               # real Rust Labby + LiveView + Chrome
just liveview-browser-fixtures  # controlled API-fixture browser checks
```

Install dependencies with `mix deps.get`, `npm ci`, and
`npx playwright install chromium` in `apps/liveview` first. Select the pinned Rust
toolchain and native build prerequisites for the real backend run. An installed
Chrome can be selected with `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH`.

`npm run test:e2e` in this directory runs the same real backend suite. The default
Playwright configuration also points to the real backend. Fixture checks have the
explicit name `npm run test:browser:fixtures` and use ports 4198/4199; they provide
controlled failure and expiry coverage, not real-backend integration evidence.

## Authenticated inventory slice

`/gateway` is a protected LiveView. Both initial HTTP mount and WebSocket mount
validate the caller's gateway session. Inventory requests use the caller's cookie
and freshly inspected CSRF token with `POST /v1/gateway`, action `gateway.list`.
This action reads the scoped snapshot; it does not trigger discovery. Responses
are bounded, schema checked, and projected to identity/enabled/connected fields.
No credential-bearing configuration is rendered. Refresh clears data on failure;
401/403 invalidates local access and redirects to sign-in. The LiveView also
redirects at the session's reported expiry. Protected pages use no-store headers.

Regression coverage includes unknown callers, malformed/oversized/schema-invalid
responses, empty inventory, async loading and expiry on refresh. Playwright adds
an inventory journey covering empty/error/recovery/forbidden states and mobile
width, alongside the existing authentication journeys. These tests use an isolated
HTTP contract fixture, not a production gateway or identity-provider qualification.

## Additional read-only slices

Server identity links open `/gateway/servers/:id`; Catalog links use the upstream
name at `/gateway/servers/:name/catalog`. Details project scoped identity, status,
and capability counts. Catalog shows retained tools, resource URIs, and prompts;
these endpoints do not provide skills and do not trigger discovery or execution.
`/diagnostics` reads `doctor` action `system.checks` without running repairs.

Each slice includes HTTP contract regressions, LiveView state tests, and a
Playwright journey for caller isolation, mobile layout, failure recovery, and
revoked access. See `docs/*-slice.md` for the precise API contracts and limits.

## Runtime and observed-client slices

`/runtime` samples `gateway.host.metrics`; `/clients` reads
`gateway.clients.list`; inventory Runtime links open
`/gateway/upstreams/:name/runtime` using the passive `gateway.mcp.list` snapshot.
These API actions require gateway administrator authority. The gateway enforces
that authority for the current caller; the console does not elevate access.

Host sampling takes about five seconds. Only that action receives an eight-second
read timeout; connect timeout, response size limits, and no-retry policy remain.
Missing metrics remain unavailable rather than becoming zero. Client observations
are history, not proof of active connections. Process snapshots may be stale.
OAuth status is deliberately excluded because its handler can refresh credentials
and trigger discovery. See the three corresponding `docs/*-slice.md` receipts.

## Usage, activity, and server logs

`/usage` reads exact `gateway.usage.metrics` aggregates over an explicit UTC window;
`/activity` reads bounded `gateway.usage.calls` pages with opaque next cursors.
Activity page counts are retained rows, not aggregate totals. Both screens offer
supported window filters and clear previous results when a refresh fails.

`/logs` queries `server_logs.query` for Labby's rolling server process logs. It is
not an upstream stdout viewer or an activity/trace table. Follow performs bounded,
sequential two-second polling and scrolls to the latest returned line. Pause stops
polling. The viewer caps each snapshot at 200 entries and the server scan at
32 KiB, within the transport's 64 KiB response cap. Filtered scan truncation is
shown explicitly. This API is query based; no native streaming endpoint is claimed.
All three actions require gateway administrator authority and use caller sessions.
See `docs/usage-metrics-slice.md`, `docs/usage-calls-slice.md`, and
`docs/server-logs-slice.md` for exact contract and TDD receipts.

## Real Labby browser integration

```sh
# Select the Rust version pinned by ../../rust-toolchain.toml first.
# Native Rust prerequisites include libclang, OpenSSL headers, pkg-config, cmake.
cd ../..
just liveview-e2e
```

The pretest hook builds frontend assets and the current Rust `labby` binary with `--locked` and checks
the pinned Rust version. Playwright starts a real loopback daemon on 4299 and the
actual LiveView app on 4298. It uses a private temporary `LABBY_HOME`, a separate
log directory, a fresh synthetic bearer credential, and the existing local owner
bootstrap command. Ambient gateway/provider credentials are not inherited.
A disabled upstream and an enabled local MCP protocol peer supply real configured
server rows, discovery, execution, and usage history without third-party requests. Tests exercise HTTP auth and authorization, server details,
retained catalog, runtime, clients, metrics, calls, logs and logout through a real
browser. The API fixture on 4199 is not involved in this suite.

Both owned processes stop after the run. Ports must be free; existing servers are
never reused. Private startup logs and disposable state are retained under the
printed diagnostics directory for failures; do not upload that directory because
it includes the synthetic credential and access database. Browser traces are off.
`LABBY_E2E_BINARY=/absolute/path/to/labby` explicitly selects a prebuilt binary and
skips the source build; such a run proves that binary only. The default command
builds source. CI runs fixture and real-backend suites as separate jobs.

These tests do not qualify external OAuth providers or third-party upstream
services. Actual local MCP execution and populated retained usage are covered by
the expanded suite below. Controlled failure/expiry scenarios remain covered by
the fast fixture suite.

### Expanded real coverage

The default suite now has independent feature journeys, a real local MCP peer,
populated tool-call history and cursor pagination, daemon-restart persistence,
and real restricted-credential/CSRF/revocation coverage. Desktop/mobile alignment
checks attach screenshots of ten implemented screens. Startup diagnostics are
bounded and redacted. See [coverage and limits](docs/real-e2e-coverage.md) and
[the mock alignment audit](docs/mock-alignment-audit.md).
