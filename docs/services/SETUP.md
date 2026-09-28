---
title: "Setup Service"
created: "2026-08-18"
updated: "2026-09-18"
---

# Setup Service

The `setup` service owns Labby's first-run, configuration, repair, proxy-configuration, and host-provisioning workflows. It is always compiled and is exposed through CLI, MCP, HTTP API, and the web UI.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact action names, parameters, destructive flags, scopes, and surface availability.

## Responsibilities

- bootstrap a new Labby home and supported host runtime
- inspect setup state
- stage, commit, and discard configuration drafts
- expose schema-driven settings state and mutations
- configure the direct stdio MCP proxy
- repair supported setup state
- project observational access-store health into setup checks without owning access-store repair

## Google OAuth and ChatGPT web

Labby's supported ChatGPT web connection requires the server to run in OAuth mode and to have a publicly reachable HTTPS origin. A bearer-only Labby server is appropriate for local or explicitly token-configured CLI clients, but it is not the supported ChatGPT web connection path.

For Google OAuth, choose the final public Labby origin before creating provider credentials. If the public origin is `https://labby.example.com`, Labby's Google callback is exactly:

```text
https://labby.example.com/auth/google/callback
```

Configure Google in this order:

1. Open the project in Google Auth Platform and complete the Branding page with the application name and support/contact information.
2. Choose the Audience appropriate for the deployment. For an External application that is still in testing, add the Google accounts that must be able to sign in as test users.
3. Under Clients, create an OAuth 2.0 client of type **Web application**.
4. Add the exact Labby callback URL above to **Authorized redirect URIs**. Do not add a wildcard, path variant, query string, or trailing slash after `callback`.
5. Copy the generated Client ID and Client secret. Treat the secret as a credential and do not put it in `config.toml`, shell history, or documentation.
6. Use the Google scopes Labby expects: `openid`, `email`, and `profile` (the defaults).
7. Start Labby setup and select Google OAuth. Interactive setup now prints the exact redirect URI before asking for the provider credentials.

Interactive setup:

```bash
labby setup
# role: Server
# authentication: OAuth + bearer break-glass
# OAuth provider: Google
# public URL: https://labby.example.com
```

Unattended setup keeps provider secrets in environment variables rather than command-line arguments:

```bash
export LABBY_GOOGLE_CLIENT_ID='...apps.googleusercontent.com'
export LABBY_GOOGLE_CLIENT_SECRET='...'
export LABBY_AUTH_ADMIN_EMAIL='operator@example.com'

labby setup \
  --role server \
  --auth both \
  --oauth google \
  --public-url https://labby.example.com \
  --no-desktop \
  --yes
```

The configured admin email must be the verified Google identity that should receive the initial administrative scope. Labby generates the remaining local OAuth encryption material during setup. Google access and refresh tokens remain server-side.

After the service is reachable through HTTPS, verify discovery before adding ChatGPT:

```bash
curl -fsS https://labby.example.com/.well-known/oauth-authorization-server
curl -fsS https://labby.example.com/.well-known/oauth-protected-resource
curl -i https://labby.example.com/mcp
```

An unauthenticated `/mcp` request should challenge the caller and point it at Labby's OAuth resource metadata. The reverse proxy must pass the OAuth discovery endpoints and `POST /register`, `/authorize`, `/token`, and `/mcp` to Labby. A WAF or proxy that blocks dynamic client registration can make ChatGPT discovery appear to work while authorization fails; use the DCR diagnostics in [OAuth](../runtime/OAUTH.md) when that happens.

In ChatGPT web, create a custom MCP app in the workspace's developer/app settings, use `https://labby.example.com/mcp` as the MCP endpoint, select OAuth, scan tools, and complete the Google authorization flow. Availability and exact ChatGPT UI labels are workspace/plan dependent and may change; Labby's durable requirement is OAuth plus the public HTTPS `/mcp` endpoint.

## macOS server and automatic updates

On Apple Silicon, the persistent server can own its update schedule. This uses
one per-user LaunchAgent for both the server and its daily update check.

After installing the Labby binary and GitHub CLI (`gh`), run this command from
a trusted repository checkout:

```bash
LABBY_SERVICE_AUTO_UPDATE=1 bash scripts/install-macos-service.sh install
bash scripts/install-macos-service.sh status
```

The installer enables `labby serve --auto-update`, preserves a stable `LABBY_HOME`
(default `~/.labby`), and records the executable search path for release verification.
After the server passes its health check, it removes `net.labby.auto-update`, the
standalone updater job. Existing user configuration and durable state remain in
`LABBY_HOME`. If startup or the scheduler handoff fails, the installer removes
the new combined service and restores the prior service state.

The server checks 60 seconds after startup and then every 24 hours. An unsuccessful
check leaves the server running until the next check. Installation has a 15-minute
deadline. Stopping the server cancels an in-flight installer before another update
can start. The next automatic check recovers any interrupted activation offline
before reading the installed version or contacting the release service. A dry run
reports required recovery without changing the installation.
A verified newer stable release replaces the executable atomically. The server stops accepting connections,
allows existing requests up to 30 seconds to finish, and exits. launchd restarts
the updated executable. Long-lived connections must reconnect after the restart.
The update applies to the local executable, not Incus containers.

Server and updater messages use `~/.labby/serve.log` and `~/.labby/serve.error.log`
(or the configured `LABBY_STATE_DIR`). The existing verified installer retains
its rollback receipt under `<install-dir>/.labby-install/`. Manual, automatic,
and direct installer entry points share one process-level transaction lock. The
installer flushes each activation boundary before advancing the journal and
retains only the current and immediately previous verified executable artifacts.

To disable automatic updates while keeping the server:

```bash
LABBY_SERVICE_AUTO_UPDATE=0 bash scripts/install-macos-service.sh install
```

Automatic updates are opt-in. Without `LABBY_SERVICE_AUTO_UPDATE=1`, installation
does not enable them. Run `serve --auto-update` only under a supervisor configured
to restart the process after a successful exit. It is unsupported for stdio MCP.

For CLI-only installations, `labby host update auto enable` installs a separate
daily LaunchAgent instead. Use `labby host update auto disable` to remove it.
macOS can retain old labels in Login Items after their LaunchAgent files are removed;
the remaining service files determine what can start at login.

## Safety Model

Read-only discovery actions such as `check`, `help`, `schema`, and `schema.get` do not require destructive confirmation. Mutating setup actions are classified as destructive and require `lab:admin` where the action catalog says so.

Plugin lifecycle and other local host mutations are additionally constrained by the product's local-action policy. Surface adapters must use the shared setup dispatcher rather than reimplementing setup behavior.

### Config validation

`setup check` and `setup repair` include a blocking `config` check. It loads `config.toml` through the same loader as `labby serve` and runs the startup validations that can stop serve or start it with a subsystem unavailable: Public Depot acquisition binding, local Depot credentials, Depot host policy, the Artifact control plane, and Skill Library exact-source adapter construction. It starts no listeners and makes no network calls. Adapter staging uses a temporary directory, never `LABBY_HOME`. On failure, `message` lists each problem as `fatal: <error chain>` (serve exits) or `degraded: <error chain>` (serve starts with Artifact services unavailable, or with the named `[[artifacts.sources]]` entry disabled on every Artifact path, for example because its `pinned_addresses` are not authorized for its host). `doctor system.checks` reports the same validation as `config:startup-validation`.

### Access-store projection

`setup check` and the check phase of `setup repair` include an `access_store` check derived from the same read-only health inspection as `doctor access.check`:

- `ready` passes.
- `missing` and `uninitialized` are advisory while access enforcement is disabled; the operator must use the explicit owner-bootstrap workflow before enabling enforcement.
- `insecure`, `corrupt`, `newer_schema`, `locked`, `read_only`, and `unavailable` are blocking failures.

`setup repair` never creates, migrates, bootstraps, chmods, checkpoints, or repairs `access.db` or its SQLite sidecars. Access-store recovery requires an explicit access-control workflow so setup repair cannot silently change authorization state or ownership.

Access-bootstrap proof, credential, identity, and journal files are bounded to
1 MiB each. Reads verify private ownership, file type, and hard-link count before
loading content. Windows publication applies the owner-only policy before writing
bytes and publishes the completed file without overwriting an existing artifact.
Recovery verifies the content digest plus the full file and parent-directory
identities before deleting through the verified Windows handle. Junctions,
alternate data streams, replaced files/parents, and inherited or foreign access
rules on files are refused; no pathname-delete fallback is used. New bootstrap
directories are made private. Existing parent directories may inherit rules,
but their owner and all write/delete-child/ACL-change authority must be limited
to the current user, Windows SYSTEM, or local Administrators. Unsafe existing
parents are refused without rewriting their permissions. Windows files are flushed
before atomic publication, but the platform does not provide the Unix parent
directory synchronization guarantee through the portable filesystem API.

## Main Action Families

| Family | Examples |
| --- | --- |
| Bootstrap and repair | `bootstrap`, `check`, `repair`, `finalize` |
| Draft configuration | `draft.get`, `draft.set`, `draft.commit`, `draft.discard` |
| Settings | `settings.state`, `settings.schema`, `settings.env_schema`, `settings.update`, `settings.env.update`, `settings.config.update` |
| Proxy | `proxy.configure` |
| Setup state | `state` |

Claude Code plugin lifecycle and plugin-hook compatibility actions are retired.
The checked-in Claude plugin is a client connection package only; it does not
configure or repair the Labby server host. Capability-affecting configuration
problems are surfaced through `doctor capabilities.status`, Doctor, and the
global admin warning surface.

## CLI

`labby setup` is the supported operator entrypoint. Use `labby setup --help` and the generated [CLI help](../generated/cli-help.md) for the exact current command grammar.

Bare `labby setup` prompts for a server or client role. Server setup uses a native
service by default; Linux x86_64 hosts with a reachable Incus daemon can select
`--deployment incus`. A server binds to `127.0.0.1:8765` unless explicitly changed.
Server authentication topology is selected independently from the OAuth provider: `--auth bearer` uses only the generated static credential, `--auth oauth` uses OAuth without a static bearer, and `--auth both` uses OAuth plus a generated static bearer break-glass credential. OAuth then selects exactly one inbound provider with `--oauth google|authelia`; Google and Authelia are alternatives, not simultaneous providers. Provider configuration requires credentials, a bootstrap admin email, and a public URL. Existing invocations that pass `--oauth google|authelia` without `--auth` retain the historical OAuth + bearer behavior for compatibility. Switching a server back to bearer-only setup clears the inactive OAuth provider, bootstrap-admin, and provider client credential entries from the protected `.env`.
Client setup saves the selected gateway URL and uses browser OAuth or a bearer
token. The optional desktop app is off by default in the interactive prompt;
`--desktop` or `LABBY_SETUP_DESKTOP=1` selects it. It is downloaded from the
matching release and its provenance is verified before installation. When no
published desktop package exists for the platform or version, or its download
or verification fails, setup still succeeds because the server or client
configuration is already complete: the summary reports `desktop_installed:
false` with `desktop_error` naming the reason, and a warning goes to stderr.

For a fresh bearer-only server, explicit setup creates the durable first owner
for its static credential. It preserves an existing owner and refuses a blocked
access store. OAuth deployments retain their authenticated owner-bootstrap flow.
Browser token sign-in exchanges the configured bearer for an HttpOnly session
cookie; the bearer is not retained by the browser, and restarting Labby invalidates
those derived sessions. Mixed browser identities must be signed out before
switching authentication methods. Browser token sign-in is offered only over
HTTPS or a direct loopback connection: a bearer-only server reached over plain
HTTP from another host, or through a reverse proxy that does not terminate TLS,
does not show the token form and refuses the exchange with `forbidden`. When a
TLS-terminating proxy fronts Labby, set `LABBY_PUBLIC_URL=https://labby.example.com`
so the exchange is accepted behind it. Bearer tokens presented directly in the
`Authorization` header by CLI and MCP clients are unaffected.

Run setup as your ordinary user. Native Linux setup requests elevation for the
service portion, then installs an optional desktop app as the original user.
OAuth client setup requires a browser; `--no-browser` rejects that combination
before changing configuration. A bearer client can be configured without a browser.

```bash
# Inspect a native bearer-only server setup without changing host state.
labby setup --role server --auth bearer --oauth none --no-desktop --yes --dry-run

# Inspect OAuth-only Google setup. Provider credentials remain in protected env vars.
labby setup --role server --auth oauth --oauth google --public-url https://labby.example.com --no-desktop --yes --dry-run

# Preserve OAuth plus the generated static bearer break-glass credential.
labby setup --role server --auth both --oauth authelia --public-url https://labby.example.com --no-desktop --yes --dry-run

# Connect this machine to an existing OAuth gateway.
labby setup --role client --server-url https://labby.example.com --oauth google --no-desktop --yes

# Keep the existing web setup entrypoint.
labby setup wizard
```

The Linux/macOS release installer invokes this same setup flow after verifying
and installing the binary. This contract requires an installer-bearing release that includes the first-run role interface; public `v1.13.3` predates it and rejects `labby setup --role ...`. The verified installer also requires an attestation-capable, authenticated GitHub CLI; Ubuntu 26.04's packaged `gh 2.46.0` is too old. Verify `gh attestation verify --help` and `gh auth status --hostname github.com` before bootstrap. Unattended callers must select `LABBY_SETUP_ROLE`;
other shell options include `LABBY_SETUP_DEPLOYMENT`, `LABBY_SETUP_HOST`,
`LABBY_SETUP_PORT`, `LABBY_SETUP_SERVER_URL`, `LABBY_SETUP_PUBLIC_URL`,
`LABBY_SETUP_AUTH`, `LABBY_SETUP_OAUTH`, `LABBY_SETUP_DESKTOP`, and
`LABBY_SETUP_NO_BROWSER`.
Provider/client secrets use the normal credential environment variables, not
command-line arguments. Set `LABBY_INSTALL_NO_SETUP=1` to install only the binary.
Updates set this flag automatically so they cannot restart onboarding.

## Related Docs

- [Configuration](../runtime/CONFIG.md)
- [Environment](../runtime/ENV.md)
- [Host gateway runtime](../runtime/HOST_GATEWAY.md)
- [Incus runtime](../runtime/INCUS.md)
- [Plugins](../PLUGINS.md)
- [Service model](../dev/SERVICES.md)
