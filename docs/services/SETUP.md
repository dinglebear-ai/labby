---
title: "Setup Service"
created: "2026-08-18"
updated: "2026-09-15"
---

# Setup Service

The `setup` service owns Labby's first-run, configuration, repair, plugin-lifecycle, proxy-configuration, and host-provisioning workflows. It is always compiled and is exposed through CLI, MCP, HTTP API, and the web UI.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact action names, parameters, destructive flags, scopes, and surface availability.

## Responsibilities

- bootstrap a new Labby home and supported host runtime
- inspect setup state and service status
- stage, commit, and discard configuration drafts
- expose schema-driven settings state and mutations
- configure the direct stdio MCP proxy
- install, uninstall, inspect, and synchronize the checked-in Claude plugin integration
- repair supported setup state
- project observational access-store health into setup checks without owning access-store repair

## Google OAuth and ChatGPT web

Labby's supported ChatGPT web connection requires OAuth and one browser-visible public HTTPS origin. Bearer auth remains useful for local/CLI and break-glass access, but it is not the supported Labby -> ChatGPT web path.

### Fast path: let Labby calculate the provider values

Before opening Google Cloud, choose the final Labby URL and ask Labby to print the exact provider recipe:

```bash
labby setup google-oauth --public-url https://labby.example.com
# Add --json for automation or ticket/checklist generation.
```

For `https://labby.example.com`, the Google callback is exactly:

```text
https://labby.example.com/auth/google/callback
```

Do not substitute the internal container URL, an IP address, the `/mcp` URL, or a URL with an extra trailing slash. Google compares the callback exactly.

### Google Cloud Console: exact click-by-click setup

1. **Sign in and select the project.** Open <https://console.cloud.google.com/>. Use the project selector in the top bar. Create a dedicated Labby project if your team does not already have one. Keep production and experiments in separate projects when practical.
2. **Open Google Auth Platform.** Use **Navigation menu -> Google Auth Platform** or go directly to <https://console.cloud.google.com/auth/overview>. If the project has never used Google Auth Platform, click **Get started**.
3. **Initial app registration.** Enter:
   - **App name:** `Labby` (or an environment-qualified name such as `Labby - Team`)
   - **User support email:** a monitored team/admin address
   - **Audience:** prefer **Internal** when the Cloud project belongs to the same Google Workspace/Cloud Identity organization as every intended Labby user; otherwise choose **External**
   - **Contact information:** a monitored team/admin address
4. **Branding.** Open <https://console.cloud.google.com/auth/branding>. Confirm the app name and support/contact information. If Google asks for **Authorized domains**, add the organization-owned registrable parent domain used by the public Labby host (for `labby.example.com`, that is normally `example.com`). Domain verification/publishing requirements are Google-owned and are more relevant to an External production app.
5. **Audience.** Open <https://console.cloud.google.com/auth/audience>.
   - **Internal** is the lowest-friction team deployment when all coworkers are in the same Workspace organization. Users outside that organization will receive `org_internal`.
   - For **External**, Testing is sufficient for a bounded deployment. Labby requests only the OpenID identity scopes `openid`, `email`, and `profile`. Google explicitly exempts that identity-only subset from the normal Testing requirement to add every user to the test-user list and from the normal seven-day Testing authorization expiry. If you later add any non-identity Google scope, revisit test users, verification, and token-expiry requirements.
6. **Data Access.** Open <https://console.cloud.google.com/auth/scopes>. Click **Add or remove scopes** if needed. Keep the app identity-only. Confirm only:
   - `openid`
   - `email` (the console may display `https://www.googleapis.com/auth/userinfo.email`)
   - `profile` (the console may display `https://www.googleapis.com/auth/userinfo.profile`)

   Labby sign-in does **not** need Gmail, Drive, Calendar, Contacts, or other Google API scopes. Do not enable unrelated Google APIs merely for Labby authentication.
7. **Create the OAuth client.** Open <https://console.cloud.google.com/auth/clients> and click **Create client**. Set:
   - **Application type:** **Web application**
   - **Name:** `Labby` or `Labby <environment>`
   - **Authorized JavaScript origins:** leave empty for Labby's server-side Google OAuth flow
   - **Authorized redirect URIs:** click **Add URI** and paste the exact callback printed by `labby setup google-oauth`, for example `https://labby.example.com/auth/google/callback`
8. Click **Create**. Copy the **Client ID** and **Client secret** immediately and put the secret in the team's secret manager. Do not commit it, paste it into tickets/chat, place it in `config.toml`, or pass it as a command-line argument. Google notes that OAuth client changes can take several minutes, and sometimes longer, to propagate.

### Configure Labby with the Google client

Interactive server setup asks for the public URL first, prints the exact Google console recipe and callback, then prompts for the client credentials:

```bash
labby setup
# Role: Server
# Deployment: Native or Incus
# Authentication: Google OAuth
# Public browser / OAuth URL: https://labby.example.com
# Google client ID: <paste client ID>
# Google client secret: <secret prompt; input is hidden>
# Bootstrap admin email: <the Google identity that should initially administer Labby>
```

For unattended deployment, keep secrets out of argv:

```bash
export LABBY_GOOGLE_CLIENT_ID='...apps.googleusercontent.com'
export LABBY_GOOGLE_CLIENT_SECRET='...'
export LABBY_AUTH_ADMIN_EMAIL='operator@example.com'

labby setup \
  --role server \
  --oauth google \
  --public-url https://labby.example.com \
  --no-desktop \
  --yes
```

Setup persists the provider credentials in Labby's managed environment file with the service configuration; the client secret is not a normal `config.toml` setting. The configured admin email must resolve to the Google identity that should receive the initial administrative scope. Labby generates its remaining local OAuth encryption/session material. Google access/refresh tokens remain server-side.

### Recommended HTTPS fast path: Tailscale Funnel

When the Labby host is already signed into Tailscale, the built-in Funnel setup is the lowest-friction way to create the public HTTPS origin ChatGPT needs. Inspection is read-only:

```bash
labby setup tailscale-funnel
```

Apply the recommended mapping to the local Labby listener with:

```bash
labby setup tailscale-funnel --apply
```

Labby accepts only loopback backends and Tailscale's public HTTPS ports `443`, `8443`, and `10000`. It will not overwrite a Funnel mapping owned by another service, and it will not silently promote an existing tailnet-only `tailscale serve` mapping to public Funnel. Configure and disable are local-only administrative actions.

On a node that has never enabled HTTPS/Funnel, Tailscale may require a one-time web approval. Labby bounds the CLI call instead of waiting indefinitely, surfaces the approval URL when Tailscale provides one, and leaves the mapping unconfigured. Complete the Tailscale approval, then rerun `labby setup tailscale-funnel --apply`. A successful result prints both exact values needed by the remaining setup:

- Google callback: `<public-origin>/auth/google/callback`
- ChatGPT MCP endpoint: `<public-origin>/mcp`

Use `--https-port 8443` or `--https-port 10000` only when you deliberately need one of Tailscale's alternate supported Funnel ports.

### Publish the HTTPS origin

The reverse proxy, tunnel, or ingress must preserve the external origin and forward all of these to the same Labby service:

- `/.well-known/oauth-authorization-server`
- `/.well-known/oauth-protected-resource`
- `/.well-known/openid-configuration` when advertised
- `/auth/login`
- `/auth/google/callback`
- `POST /register`
- `/authorize`
- `/token`
- `/mcp`

Do not protect those OAuth protocol routes behind a second incompatible authentication layer. A WAF/proxy that blocks Dynamic Client Registration can make MCP discovery succeed while ChatGPT authorization later fails.

### Prove Google -> Labby before adding ChatGPT

Run this ladder in order:

```bash
# 1. OAuth discovery is public and coherent.
curl -fsS https://labby.example.com/.well-known/oauth-authorization-server
curl -fsS https://labby.example.com/.well-known/oauth-protected-resource

# 2. MCP exists. An unauthenticated request should challenge rather than 404.
curl -i https://labby.example.com/mcp
```

Then open this URL in a browser and complete Google sign-in:

```text
https://labby.example.com/auth/login?return_to=%2F
```

A successful browser flow must return through the exact Google callback and leave you signed into Labby as the expected identity. If Google reports `redirect_uri_mismatch`, compare the URI character-for-character and allow for provider propagation time. If Google reports `org_internal`, the selected Google account is outside an Internal app's organization.

### Connect Labby to ChatGPT web

Once the browser login succeeds:

1. Use **ChatGPT web**. Full custom MCP app setup is currently a web workflow.
2. Ensure developer mode is available to your account. Workspace admins first enable it under **Workspace Settings -> Permissions & Roles -> Connected Data Developer mode / Create custom MCP connectors**. On Enterprise/Edu, an enabled coworker then turns it on under **Settings -> Apps -> Advanced Settings**. Business admins/owners can enable it for themselves while creating an app under **Workspace Settings -> Apps -> Create**.
3. Create the app. Admins/owners can use **Workspace Settings -> Apps -> Create**. An authorized user can use **Settings -> Apps -> Create**.
4. Enter Labby's MCP endpoint: `https://labby.example.com/mcp`.
5. Choose **OAuth** as the authentication mechanism.
6. Click **Scan Tools** and wait for discovery. ChatGPT should read Labby's protected-resource and authorization-server metadata.
7. Complete the OAuth prompt with an allowed Google coworker identity, then let **Scan Tools** finish.
8. Click **Create**. In user settings the app appears under **Settings -> Apps -> Enabled Apps** with a **Dev** label; workspace-created apps appear as drafts under **Workspace Settings -> Apps -> Drafts** until an admin/owner publishes them.
9. Open a new chat, select the draft/dev Labby app from the tools menu, and perform one safe read-only call. Confirm the expected tool surface before publishing or enabling write-capable actions broadly.

If discovery works but authorization fails, verify `/register`, `/authorize`, `/token`, and the WAF/proxy path before changing Google. Use [OAuth](../runtime/OAUTH.md) for DCR/CIMD diagnostics.

### Google OAuth troubleshooting by symptom

| Symptom | Most likely cause | Fix |
| --- | --- | --- |
| `redirect_uri_mismatch` | Google client callback differs from Labby | Copy the value from `labby setup google-oauth --public-url ...` exactly; no wildcard/query/trailing slash |
| `org_internal` | External Google account against an Internal app | Use an account in the project organization or change Audience deliberately |
| Google client ID/secret rejected immediately | Wrong client, secret, or stale change | Re-open **Google Auth Platform -> Clients**, verify the Web application client, then allow propagation time |
| Login works in browser but ChatGPT cannot authorize | Public OAuth/DCR routes blocked or rewritten | Verify discovery, `/register`, `/authorize`, `/token`, and `/mcp` through the public edge |
| OAuth redirects to an internal hostname | `LABBY_PUBLIC_URL` does not match the real browser origin | Re-run setup with the final HTTPS origin |
| Google asks for unrelated Gmail/Drive permissions | Extra Google scopes were configured | Remove them; Labby authentication needs only `openid email profile` |

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

For CLI-only installations, `labby update --auto-update enable` installs a separate
daily LaunchAgent instead. Use `labby update --auto-update disable` to remove it.
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
| Plugin lifecycle | `plugin.install`, `plugin.uninstall`, `plugins.installed`, `plugin_hook`, `plugin_sync`, `plugin_export` |
| Local stdio proxy | `proxy.configure` |
| Public HTTPS | `public_proxy.render` |
| Organization enrollment | `organization_profile.create`, `organization_profile.preview`, `organization_profile.apply` |
| Service inspection | `services.status`, `state` |

Legacy snake-case plugin action aliases remain in the action catalog for compatibility; new integrations should use the dotted canonical action names.

## Organization Bootstrap Profiles

Team Labby can publish a signed, non-secret profile that seeds approved team
integrations into a teammate's personal Labby after enrollment. The profile is
data, not remote runtime authority: applying it mutates only the teammate's
personal gateway configuration, and later execution remains controlled by that
personal Labby.

- `organization_profile.create` signs the canonical profile with the existing
  Depot authority Ed25519 signing seed. The operator supplies an externally
  reachable HTTPS Team Depot MCP URL plus a public key id. The profile always
  includes Team Depot and the Linear notifications MCP; it contains no bearer
  tokens, OAuth refresh tokens, client secrets, or local container addresses.
- `organization_profile.preview` verifies the signature and returns the signer
  SHA-256 fingerprint plus the exact integrations that would be added. It does
  not mutate gateway state.
- `organization_profile.apply` requires an explicit
  `expected_signer_fingerprint` and verifies the signature again. Existing
  personal definitions with the same canonical name + endpoint are preserved;
  a same-name/different-endpoint conflict aborts before mutation. Missing Team
  Depot and Linear definitions use the gateway's all-or-nothing batch path, so
  the organization defaults are persisted together or not at all. On first
  trust, the fingerprint is a human trust decision: Team Labby shows it before
  handoff, personal Labby shows the cryptographically verified fingerprint
  again, and the user explicitly confirms trust before apply. Signature
  verification proves integrity and possession of that key; it does not by
  itself make an unknown self-signed key a pre-pinned organization root.

A Team Labby that sets `LABBY_ORGANIZATION_BOOTSTRAP_TEAM_DEPOT_URL` and
`LABBY_ORGANIZATION_BOOTSTRAP_KEY_ID` will attach this signed profile offer to
a successful email-bound Team invitation acceptance when the signing key is
available. The signed profile carries the accepted membership's real
Organization ID. Failure to construct the optional profile is logged but never
rolls back or masks a successful Team enrollment.

The browser completion flow does not apply that profile to Team Labby. It offers
a safe handoff to the user's personal Labby (local `127.0.0.1:8765` by
default, or a remote HTTPS origin). The non-secret signed profile travels only
in the URL fragment, survives personal-Labby sign-in in same-tab session
storage, and is then previewed on personal Labby. The user sees the exact
integrations and signer fingerprint and explicitly chooses **Trust team and
apply defaults**. Skipping the profile leaves Team membership intact and changes
nothing in personal runtime configuration.

## Public HTTPS without reverse-proxy guesswork

`public_proxy.render` is the shared non-mutating contract for public exposure.
It validates an HTTPS public origin plus a credential-free HTTP(S) private
backend origin, then renders streaming-safe Caddy, Nginx, and Traefik examples
and the exact verification commands. Caddy is the recommended low-friction
default. The WebUI exposes the same contract under **Settings → Doctor → Public
HTTPS**; Nginx and Traefik stay behind progressive disclosure. The existing
`proxy.configure` action is a different capability: it configures Labby's
ephemeral stdio MCP proxy and is not a public reverse-proxy generator.

## CLI

`labby setup` is the supported operator entrypoint. Use `labby setup --help` and the generated [CLI help](../generated/cli-help.md) for the exact current command grammar.

Bare `labby setup` starts with intent, not infrastructure trivia. After choosing
whether this computer runs Labby or connects to another Labby, a new server gets
three choices: **Personal / local**, **Browser + ChatGPT**, or **Customize**.

- **Personal / local** uses the secure native defaults (`127.0.0.1:8765`, bearer
  authentication, desktop app when supported) without asking deployment, bind,
  port, auth-provider, or desktop questions.
- **Browser + ChatGPT** uses the same network/deployment defaults and selects
  Google OAuth, then asks only for the public HTTPS/provider values actually
  required for browser authentication.
- **Customize** preserves every existing advanced control: native vs Incus, bind
  address, port, bearer/Google/Authelia, desktop selection, and the explicit CLI
  flags. Supplying those advanced flags directly also selects the matching path.

Resumable setup keeps staged host, port, and authentication choices, so an
interrupted run continues instead of silently starting over. Client setup saves
the selected gateway URL and uses browser OAuth or a bearer token. The two
recommended server experiences select the desktop app automatically when this
platform supports it; Customize and client setup keep the explicit desktop
choice, whose interactive prompt defaults to No. Published desktop packages
remain provenance-verified before installation. A missing, failed, or unverifiable
desktop package never rolls back an otherwise-complete server/client setup; the
summary reports `desktop_installed: false` and includes the reason.


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
# The common interactive path: choose Personal/local, Browser + ChatGPT, or Customize.
labby setup

# Inspect an explicit native/bearer server plan without changing host state.
labby setup --role server --oauth none --no-desktop --yes --dry-run

# Generate the recommended Caddy reverse-proxy config for a public Labby origin.
labby setup public-proxy --public-url https://labby.example.com

# Power users can render Nginx, Traefik, or every supported proxy format.
labby setup public-proxy --public-url https://labby.example.com --format nginx

# Connect this machine to an existing OAuth gateway.
labby setup --role client --server-url https://labby.example.com --oauth google --no-desktop --yes

# Keep the existing web setup entrypoint.
labby setup wizard
```

The Linux/macOS release installer invokes this same setup flow after verifying
and installing the binary. This contract requires an installer-bearing release that includes the first-run role interface and `release-provenance.sigstore.json`; public `v1.13.3` predates it and rejects `labby setup --role ...`. The verified installer requires an attestation-capable GitHub CLI, but it verifies the published Sigstore bundle locally and therefore does not require `gh auth login` or a personal GitHub token. Ubuntu 26.04's packaged `gh 2.46.0` is too old. Verify `gh attestation verify --help` before bootstrap. Unattended callers must select `LABBY_SETUP_ROLE`;
other shell options include `LABBY_SETUP_DEPLOYMENT`, `LABBY_SETUP_HOST`,
`LABBY_SETUP_PORT`, `LABBY_SETUP_SERVER_URL`, `LABBY_SETUP_PUBLIC_URL`,
`LABBY_SETUP_OAUTH`, `LABBY_SETUP_DESKTOP`, and `LABBY_SETUP_NO_BROWSER`.
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
