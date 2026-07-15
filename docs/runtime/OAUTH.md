# HTTP Auth Modes

Lab supports two HTTP auth modes:

- `LABBY_AUTH_MODE=bearer`
  Preserve the existing static bearer-token flow with `LABBY_MCP_HTTP_TOKEN`.
- `LABBY_AUTH_MODE=oauth`
  Run an internal Google-backed OAuth authorization server that issues `lab` JWT access tokens and exposes JWKS plus RFC 9728 metadata.

This document covers mode selection, startup behavior, registration and token flow, JWT validation, and operator-facing constraints.
For the complete generated route/auth matrix, see
[generated/api-routes.md](../generated/api-routes.md).

## Configuration

OAuth mode is configured through env vars and/or `config.toml`. Env vars take precedence over config file values.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `LABBY_AUTH_MODE` | no | `bearer` or `oauth`. Defaults to `bearer`. |
| `LABBY_MCP_HTTP_TOKEN` | bearer mode | Static bearer token for protected HTTP routes. |
| `LABBY_PUBLIC_URL` | oauth mode | Public base URL for metadata, callback construction, and JWT issuer/audience. Path-prefixed deployments are supported. |
| `LABBY_GOOGLE_CLIENT_ID` | oauth mode | Google OAuth client ID. |
| `LABBY_GOOGLE_CLIENT_SECRET` | oauth mode | Google OAuth client secret. |
| `LABBY_AUTH_SQLITE_PATH` | no | Override path for the SQLite auth database. |
| `LABBY_AUTH_KEY_PATH` | no | Override path for the persisted JWT signing key. |
| `LABBY_AUTH_ALLOWED_REDIRECT_URIS` | no | Comma-separated redirect URI patterns allowed for dynamic client registration. When unset, Labby seeds common ChatGPT/Claude callback patterns. Set it explicitly to replace those defaults; use `https://*` only when the operator intentionally trusts any HTTPS DCR callback. Loopback/native-app callbacks are accepted by the auth layer. |
| `LABBY_AUTH_ADMIN_EMAIL` | oauth mode | Google email address of the bootstrap admin permitted to log in. Normalized to lowercase at startup. **Required** when `LABBY_AUTH_MODE=oauth`: startup fails if unset so no Google account can authenticate unless explicitly permitted. The `email_verified` claim in Google's id_token is enforced — accounts with unverified email addresses are rejected even if the address matches. Additional users are granted through the SQLite-backed allowlist managed from Labby settings. |
| `LABBY_GOOGLE_CALLBACK_PATH` | no | Callback path appended to `LABBY_PUBLIC_URL`. Defaults to `/auth/google/callback`. |
| `LABBY_GOOGLE_SCOPES` | no | Comma-separated Google scopes. Defaults to `openid,email,profile`. |
| `LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `POST /register`. Defaults to `20`. |
| `LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `/authorize` and browser login initiation. Defaults to `60`. |
| `LABBY_AUTH_MAX_PENDING_OAUTH_STATES` | no | Maximum non-expired pending authorization + browser-login states stored at once. Defaults to `1024`. |

## Startup Behavior

When OAuth mode is configured, `labby serve` performs these steps at startup:

1. Validate that `LABBY_PUBLIC_URL`, Google credentials, and `LABBY_AUTH_ADMIN_EMAIL` are present.
2. Open the SQLite auth store in WAL mode with a non-zero busy timeout.
3. Load or generate the persisted RSA signing key.
4. Build the concrete Google provider callback URL from `LABBY_PUBLIC_URL` and `LABBY_GOOGLE_CALLBACK_PATH`.

Startup fails closed if any of those steps fail.

Startup also fails if:

- `LABBY_AUTH_MODE=oauth` is set without `LABBY_PUBLIC_URL`
- Google client credentials are missing
- `LABBY_AUTH_ADMIN_EMAIL` is missing — fail-closed default so no Google account can authenticate without explicit permission
- the auth database or signing key has insecure file permissions

## Registration and Authorize Flow

OAuth mode exposes:

- `POST /register`
- `GET /authorize`
- `GET /auth/google/callback`
- `POST /token`

Registration rules in the initial launch:

- loopback redirect URIs are always accepted
- native-app private-use URI redirects are always accepted
- when no explicit redirect allowlist is configured, Labby seeds common ChatGPT/Claude callback patterns
- explicit `LABBY_AUTH_ALLOWED_REDIRECT_URIS` or `[auth].allowed_client_redirect_uris` values replace the product defaults
- unlisted public HTTPS redirect URIs are rejected unless a configured pattern matches them
- `https://*` is supported as an explicit operator opt-in for trusting any HTTPS DCR callback
- `POST /register`, `/authorize`, and hosted browser-login initiation are process-locally rate limited
- new login/authorization state is rejected once the pending non-expired state cap is reached

Flow summary:

1. A client registers a loopback redirect URI, a native-app URI, a product-default callback URI, or one that matches the configured allowlist.
2. The client sends the user to `/authorize` with `response_type=code`.
3. `lab` stores the request state, generates PKCE data, and redirects to Google.
4. Google redirects back to `/auth/google/callback`.
5. `lab` enforces the email allowlist (currently `LABBY_AUTH_ADMIN_EMAIL`; expanding to a SQLite-backed user list managed via the web UI). The id_token's `email_verified` claim is required — unverified accounts are rejected even when the address matches. Browser-login callers receive a 401; OAuth-client callers receive an RFC 6749 §4.1.2.1 redirect with `error=access_denied`.
6. `lab` exchanges the Google code server-side, stores a local authorization code, and redirects the client back to its registered redirect URI with the local code.
7. The client exchanges that local code at `/token` for a `lab` access token and, when Google granted offline access, a `lab` refresh token.

Google access and refresh tokens remain server-side only.

Google-specific notes:

- `lab` sends `access_type=offline` when redirecting to Google so the provider can issue a refresh token
- `lab` also sends `prompt=consent` so a fresh Google consent flow can return a new refresh token after the app was previously authorized without offline access
- if Google still does not return an upstream refresh token, `lab` omits `refresh_token` from its token response and later refresh grants fail closed
- `lab` validates the Google `id_token` cryptographically against Google JWKS and rejects tokens with the wrong issuer, audience, or expiry before minting any local identity

## Browser-Local Callback Forwarding

`lab` also ships a local OAuth callback forwarder for browser-side machines:

```bash
labby oauth relay-local --machine node-a --port 38935
labby oauth relay-local --forward-base http://node.internal.example:38935/callback/node-a --port 38935
```

This helper exists for cases where:

- the browser receives a loopback redirect on one machine
- the actual OAuth client callback listener is running on another machine
- you need to forward the final callback request without reimplementing the OAuth flow

Important constraints:

- `relay-local` binds only to `127.0.0.1:<port>` on the browser machine
- it forwards only the final callback request
- it forwards only a callback-safe header allowlist; `Cookie`,
  `Authorization`, and similar ambient credentials are stripped
- it mirrors only a callback-safe response header allowlist; `Set-Cookie` and
  other credential-bearing response headers are not relayed back through the
  localhost helper
- it does not mint tokens, store PKCE state, or complete the OAuth exchange itself
- the real client listener must already be running and reachable before the callback arrives

## Public Callback Relay

Labby can also serve the public Codex MCP OAuth callback relay at:

```text
https://callback.tootie.tv/callback/<machine>
https://callback.tootie.tv/callback/<machine>/<suffix>
```

This is for remote, headless, or cross-namespace clients whose browser cannot
reach the client's local loopback listener directly. Regular non-headless
desktop clients should keep local loopback callbacks where possible.

Client configuration example:

```toml
mcp_oauth_callback_url = "https://callback.tootie.tv/callback/dookie"
```

The public relay is transport-only. It forwards the final callback request to
the registered machine target; Codex or the MCP client still owns PKCE, state
validation, and token exchange.

Public relay constraints:

- public callback routes are unauthenticated: `GET|POST /callback/<machine>[/*suffix]`
- admin mutation lives under authenticated `/v1/oauth/relay/*` and requires `lab:admin`
- targets must be `http://<tailscale-ip>:38935/callback/<machine>` (host in the Tailscale CGNAT range `100.64.0.0/10`, e.g. `http://100.88.16.79:38935/callback/dookie`) with no userinfo, query, or fragment
- query strings, request bodies, auth headers, cookies, `code`, `state`, and full target URLs are not logged
- forwarding does not follow redirects and strips `Location` and `Set-Cookie`
- `/healthz` is shallow: process alive, relay enabled, registry loaded
- deep target reachability belongs in explicit doctor checks:

```bash
labby doctor oauth-relay --probe-targets --json
```

The registry is separate from `[oauth.machines]` and is stored at:

```text
~/.labby/oauth-public-relay/registry.json
```

Offline registry management:

```bash
labby oauth relay-registry list --json
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
labby oauth relay-registry register \
  --machine dookie \
  --target-url http://100.88.16.79:38935/callback/dookie
labby oauth relay-registry disable --machine dookie
labby oauth relay-registry enable --machine dookie
labby oauth relay-registry remove --machine dookie
```

CLI registry mutations write the sidecar file and report
`restart_required: true`; restart `labby serve` to refresh a running server's
in-memory snapshot. The authenticated admin API updates the live snapshot and
the sidecar together. Registry imports are all-or-nothing: any quarantined
machine or invalid target rejects the import without replacing the active
registry.

For the production cutover and rollback procedure, see
[CALLBACK_RELAY.md](../deploy/CALLBACK_RELAY.md).

## Codex MCP OAuth Client Setup

Codex desktop clients usually do not need callback relay settings. When the
browser and the `codex` process run on the same local machine, configure only the
MCP server URL and run the native login flow:

```toml
[mcp_servers.labby]
url = "https://labby.example.com/mcp"
```

```bash
codex mcp login labby
```

Use callback override settings only when the browser cannot reach Codex's
temporary local callback listener directly. Common examples are SSH sessions,
remote dev boxes, WSL/browser splits, dev containers, and headless Linux hosts.
In that shape, Codex still owns the PKCE state and token exchange; the relay only
transports the final browser callback to the waiting Codex process.

```toml
mcp_oauth_callback_port = 38935
mcp_oauth_callback_url = "https://callback.example.com/callback/<machine>"

[mcp_servers.labby]
url = "https://labby.example.com/mcp"
```

The public relay must also have a matching machine target that forwards to the
exact callback path on the Codex host, for example:

```text
https://callback.example.com/callback/dookie
  -> http://100.88.16.79:38935/callback/dookie
```

For Linux sessions without a usable desktop keyring or D-Bus session, prefer
file-backed MCP OAuth credentials:

```toml
mcp_oauth_credentials_store = "file"
```

This is mainly a headless/SSH workaround. Do not force it for ordinary desktop
clients where the platform credential store works.

## Node Runtime Relay Start

The same local relay can be started remotely on a fleet node through:

```http
POST /v1/nodes/oauth/relay/start
```

Example body:

```json
{
  "bind_addr": "127.0.0.1:38935",
  "target_url": "http://node.internal.example:38935/callback/node-a",
  "default_port": 38935,
  "request_timeout_ms": 30000
}
```

This reuses the existing local relay implementation. It does not change OAuth token issuance or PKCE handling.

In the current v1 trust model, this endpoint is intended for controller-orchestrated node runtime traffic on the tailnet. It is not exposed as a public operator surface on non-controller nodes; the controller invokes it after authenticating to the target node with the same shared bearer/OAuth controls that protect the rest of `/v1/*`.

### Using non-loopback redirect URIs

Loopback redirect URIs are always accepted by `lab-auth`. Native-app private-use
URI schemes such as `cursor://...`, `warp://...`, `vscode://...`, and
`com.raycast:/...` are also accepted without per-client allowlist entries.

The Labby gateway product seeds common browser-based MCP callback patterns for
ChatGPT and Claude when no explicit redirect allowlist is configured. It does
not trust every HTTPS callback by default. This keeps common ChatGPT/Claude
connectors working out of the box while preserving an operator-controlled
boundary for other public HTTPS callbacks. Arbitrary non-loopback `http://`
callbacks remain blocked.

Configure extra allowed redirect URI patterns with either:

- `LABBY_AUTH_ALLOWED_REDIRECT_URIS`
- `[auth].allowed_client_redirect_uris`

Example for an additional HTTPS callback relay:

```env
LABBY_AUTH_ALLOWED_REDIRECT_URIS=https://callback.example.com/callback/*
```

```toml
[auth]
allowed_client_redirect_uris = [
  "https://callback.example.com/callback/*",
]
```

ChatGPT custom MCP connectors use callback URLs shaped like
`https://chatgpt.com/connector/oauth/{callback_id}`. Labby's product defaults
also include the legacy ChatGPT redirect and Claude callback URLs. Other
browser-based clients, such as Gemini, VS Code, Zed, Cursor, Windsurf, Cline,
Roo Code, Kilo Code, Droid, Antigravity, OpenClaw, Hermes, and future MCP
clients, may use different HTTPS domains. Add those patterns explicitly as they
are verified, or configure `https://*` only when you intentionally accept the
risk of trusting any HTTPS DCR callback.

Patterns are matched as structured URLs, not raw substrings:

- scheme and port must match exactly
- host wildcards are allowed only as full labels, e.g. `https://*.example.com/callback` or `https://callback.*.tv/callback/*`
- path and query may use simple `*` wildcards
- partial host-label globs such as `https://callback.example.com*` are rejected and do not safely scope a trust boundary

Use this only for redirect URIs you explicitly operate or trust.

## Runtime JWT Validation

Every request to a protected route (`/v1/*`, `/mcp`) must include an `Authorization: Bearer <token>` header.

Validation steps:

1. Decode the JWT header to extract the `kid` (key ID).
2. Look up the signing key in the cached JWKS.
3. If the `kid` is unknown, trigger an eager JWKS refresh (see caching below).
4. Validate the JWT signature using one of the supported algorithms.
5. Validate the `iss` claim matches the configured issuer.
6. Validate the `aud` claim matches the configured audience.
7. Extract scopes from the `scope` claim (space-separated string) or the `scp` claim (JSON array).

### Supported Algorithm

- RS256

### Scopes

Current `lab` tokens use the standard space-delimited `scope` claim.

### AuthContext

On successful validation, an `AuthContext` is injected into the request extensions:

- `sub` — the authenticated user/client identifier from the `sub` claim.
- `scopes` — granted scopes.
- `issuer` — token issuer.

Downstream handlers can read `AuthContext` from request extensions for audit trails and scope-gated access.

## Token Exchange

`POST /token` supports:

- `grant_type=authorization_code`
- `grant_type=refresh_token`

Current constraints:

- authorization-code redemption is atomic and single-use
- `refresh_token` is only issued when Google returned an upstream refresh token
- refresh grants are rejected if the local token is not backed by an upstream refresh token
- refresh tokens do not rotate in this batch
- `/revoke` is not implemented in this batch
- successful and failed `/token` responses must send `Cache-Control: no-store`
  and `Pragma: no-cache`

### Auth Failure Semantics

`lab` distinguishes unauthenticated callers from internal auth outages.

Rules:

- `/auth/session` returns an unauthenticated result only when the request truly
  lacks a valid session
- auth store, signing-key, provider, or persistence failures stay 5xx-class and
  use canonical error envelopes
- `/auth/logout` failures are surfaced as structured errors rather than being
  treated as best-effort success
- provider-facing logs must preserve stable `kind` classification when transport,
  status, decode, or grant failures happen

Browser-session introspection semantics:

- `GET /auth/session` returns `200` with `authenticated: false` only for a true
  logged-out outcome
- the same payload includes `login_available` so browser clients can suppress
  the hosted-login CTA when OAuth browser login is not configured
- a request that carries `Authorization: Bearer <LABBY_MCP_HTTP_TOKEN>` is treated
  as an authenticated admin caller and gets `authenticated: true` with
  `sub: "static-bearer"`, `is_admin: true`, and an empty `csrf_token` (CSRF is
  unnecessary for bearer-authenticated requests). This is the bridge that lets
  automation tooling (e.g. `agent-browser --headers`) drive the UI alongside
  OAuth browser users without the flag-and-disable dance
- internal failures from session lookup, persistence, signing, or provider
  coordination remain structured 5xx responses instead of collapsing into
  `authenticated: false`

### Frontend Expectations

The web UI and server-side frontend adapter must treat auth state as a three-way
 distinction:

- `loading`
- `unauthenticated`
- `auth_error`

They must also:

- capture response `x-request-id` values on failures
- avoid showing a hosted-login CTA unless hosted login is actually available
- invalidate or refresh cached session state when later requests fail with
  `auth_failed` or a CSRF-style `validation_failed` response
- not treat unrelated validation failures as implicit logout/session-expiry events

### OAuth Error Kinds

Most auth-route failures use the canonical error envelope described in
`docs/ERRORS.md`.

Documented auth-specific exception:

- `invalid_grant` remains a stable OAuth token/authorization error for
  authorization-code and refresh-token redemption failures such as expired,
  unknown, or mismatched grants

## RFC 9728 Protected Resource Metadata

Lab exposes a metadata endpoint so MCP clients can discover which authorization server to use:

```http
GET /.well-known/oauth-protected-resource
```

This endpoint is **unauthenticated** — clients need it before they have a token.

Response:

```json
{
  "resource": "https://lab.example.com",
  "authorization_servers": ["https://lab.example.com"],
  "scopes_supported": ["lab"],
  "bearer_methods_supported": ["header"]
}
```

### WWW-Authenticate Header

When a request fails authentication (401), the response includes:

```http
WWW-Authenticate: Bearer resource_metadata="https://lab.example.com/.well-known/oauth-protected-resource"
```

This header is only included when `LABBY_PUBLIC_URL` is configured. If not, the header is omitted rather than advertising localhost.

## Gateway-Managed Route Metadata

Gateway-managed protected MCP routes publish route-specific OAuth protected
resource metadata under the public route host:

```http
GET /.well-known/oauth-protected-resource/<route-path>
```

For a route configured as `public_host = "mcp.example.com"` and
`public_path = "/telemetry"`, clients discover:

```http
GET https://mcp.example.com/.well-known/oauth-protected-resource/telemetry
```

The metadata `resource` value is the public MCP resource:

```json
{
  "resource": "https://mcp.example.com/telemetry",
  "authorization_servers": ["https://lab.example.com"],
  "scopes_supported": ["mcp:read", "mcp:write"],
  "bearer_methods_supported": ["header"]
}
```

An unauthenticated request to the route returns a route-specific challenge:

```http
WWW-Authenticate: Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/telemetry"
```

OAuth clients must request a token for the route resource
`https://mcp.example.com/telemetry` and present that token to
`https://mcp.example.com/telemetry`. The backend MCP URL remains private and must
not appear in public metadata, public challenges, or public error bodies.

Static bearer compatibility does not make a public protected MCP route an OAuth
resource credential. `LABBY_MCP_HTTP_TOKEN` is an operator/admin shortcut for
Lab admin/API surfaces; Gateway-managed public MCP routes validate Lab OAuth
JWTs whose audience is the route resource.

Disabled or unknown protected routes must not advertise protected-resource
metadata or proxy to a backend. They should fail with a stable public error that
does not leak backend origins, backend paths, private IPs, or token env var
names.

Full route configuration and curl verification examples live in
[GATEWAY.md](../services/GATEWAY.md#gateway-managed-protected-mcp-routes).

## Troubleshooting ChatGPT MCP Connectors

Use this checklist when a ChatGPT custom MCP connector fails during dynamic
client registration, OAuth, or the first MCP request after OAuth succeeds. The
important split is **which layer returned the error**: edge proxy, Labby's auth
server, Labby's protected-route auth, or the protected-route backend.

### Dynamic client registration returns 403

ChatGPT may show:

```text
Dynamic client registration failed: registration endpoint returned 403
```

First verify whether `POST /register` reached the origin. In an nginx/SWAG
front end, look for ChatGPT/OpenAI user agents around the failure time:

```bash
grep -E 'POST /register|/\.well-known/oauth|GET /mcp|POST /mcp' \
  /path/to/nginx/access.log | tail -n 80
```

Interpretation:

- `GET /.well-known/oauth-protected-resource/<path>` and
  `GET /.well-known/oauth-authorization-server` reach the origin, but
  `POST /register` is absent: the edge proxy or WAF blocked DCR before Labby
  saw it.
- `POST /register` reaches the origin and returns 4xx: inspect Labby logs and
  redirect allowlist config.
- `POST /register` reaches the origin and returns 200: DCR itself is not the
  current failure; continue to the OAuth/token/MCP checks below.

When Cloudflare proxying is enabled, a WAF/bot rule can block ChatGPT's DCR
POST while allowing metadata GETs. Confirm the origin path by bypassing
Cloudflare with `--resolve`:

```bash
WAN_IP=203.0.113.10
ISSUER=mcp.example.com

curl --resolve "$ISSUER:443:$WAN_IP" \
  -sS -D - "https://$ISSUER/.well-known/oauth-authorization-server" -o /tmp/as.json

curl --resolve "$ISSUER:443:$WAN_IP" \
  -sS -D - -X POST "https://$ISSUER/register" \
  -H 'Content-Type: application/json' \
  --data '{
    "redirect_uris":["https://chatgpt.com/connector/oauth/<callback-id>"],
    "client_name":"dcr-smoke",
    "scope":"mcp:read mcp:write",
    "grant_types":["authorization_code"],
    "response_types":["code"],
    "token_endpoint_auth_method":"none"
  }'
```

Use the actual callback URI from the failed connector when reproducing a
redirect-allowlist problem; the placeholder above is only the expected shape.

If the direct-origin POST returns 200 but ChatGPT still gets 403, fix the edge
configuration, not Labby. The simplest operational fix is to make the connector
host DNS-only instead of Cloudflare-proxied. Alternatively, add a narrow WAF
bypass for the OAuth/MCP paths used by MCP clients:

- `/.well-known/oauth-protected-resource*`
- `/.well-known/oauth-authorization-server*`
- `/.well-known/openid-configuration`
- `/register`
- `/authorize`
- `/token`
- `/mcp`

If Labby rejects the DCR POST itself, check the redirect URI ChatGPT registered
and compare it with `LABBY_AUTH_ALLOWED_REDIRECT_URIS` or
`[auth].allowed_client_redirect_uris`. Current ChatGPT custom connectors use
callbacks shaped like:

```text
https://chatgpt.com/connector/oauth/<callback-id>
```

Older flows may use:

```text
https://chat.openai.com/aip/plugin-callback
```

When `LABBY_AUTH_ALLOWED_REDIRECT_URIS` is set explicitly, it replaces product
defaults. Include both the current and legacy ChatGPT callback patterns if the
deployment needs to support both.

### OAuth completes, but ChatGPT says it cannot connect

ChatGPT may complete the browser OAuth flow, then show:

```text
There was a problem connecting <name>. Try again later.
```

Check the request sequence at the origin:

```bash
grep -E 'POST /token|POST /mcp|/\.well-known/oauth' \
  /path/to/nginx/access.log | tail -n 80
```

Common signatures:

- `POST /token` returns 200, then `POST /mcp` returns 401:
  token exchange worked; the failure is the first MCP request.
- Labby logs `protected MCP route auth failed: missing bearer token`:
  the client did not send a bearer token, or it did not discover the
  route-specific metadata challenge correctly.
- Labby logs `protected MCP route auth failed: JWT validation failed`:
  the access token issuer or audience does not match the public route resource.
- Labby logs `protected MCP route auth accepted`, then
  `protected MCP route proxy finish ... status=401`:
  Labby accepted ChatGPT's OAuth token, then proxied the request to a backend
  that rejected the unauthenticated upstream request.

The last case is easy to create accidentally when publishing a friendly root
URL. This is wrong for a route that should expose Labby itself:

```toml
[[protected_mcp_routes]]
name = "root"
enabled = true
public_host = "example.com"
public_path = "/mcp"
backend_url = "https://mcp.example.com/mcp"
scopes = ["mcp:read", "mcp:write"]
```

That configuration validates the OAuth token for `https://example.com/mcp`,
then forwards to another protected public MCP endpoint without an upstream
credential. The backend returns 401.

For a public route that should expose a scoped Labby gateway surface, use a
`gateway_subset` target instead. Gateway subsets are mounted in-process after
the public route's OAuth check, so there is no second public auth hop:

```toml
[[protected_mcp_routes]]
name = "root"
enabled = true
public_host = "example.com"
public_path = "/mcp"
scopes = ["mcp:read", "mcp:write"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["github", "quick-shell", "filesystem"]
services = ["gateway"]
expose_code_mode = true
```

Gateway-subset routes are mounted when `labby serve` starts. Editing a running
route through the live gateway may return `restart_required`; update
`config.toml` and restart the service:

```bash
systemctl restart labby.service
labby gateway protected-route get root --json
```

After restart, verify the public challenge and route metadata:

```bash
curl -sS -D - -o /tmp/mcp-unauth-body https://example.com/mcp
cat /tmp/mcp-unauth-body

curl -sS https://example.com/.well-known/oauth-protected-resource/mcp
```

Expected properties:

- `GET /mcp` without auth returns 401
- `WWW-Authenticate` points to
  `https://example.com/.well-known/oauth-protected-resource/mcp`
- protected-resource metadata has
  `"resource": "https://example.com/mcp"`
- authorization server metadata points to the issuer configured by
  `LABBY_PUBLIC_URL`

After a real connector retry, Labby service logs should show the happy path:

```text
oauth token response minted access token
protected MCP route auth accepted
initializing HTTP MCP session handler ... route_scope=protected:<route-name>
tool list ok
```

In Code Mode visibility, ChatGPT may only list one MCP tool, `codemode`, even
when many upstreams are available. That is intentional: `codemode` exposes
in-sandbox discovery and execution for the route-scoped upstream catalog.

## Auth Precedence

When both static bearer and OAuth are configured, auth is checked in this order:

1. **Static bearer token** — constant-time comparison via `LABBY_MCP_HTTP_TOKEN`. If it matches, the request is authenticated with implicit `lab:read` and `lab:admin` scopes.
2. **OAuth JWT** — if the static bearer check fails (or no static token is configured), the token is validated as a JWT against the cached JWKS. Tokens for Lab's own `/mcp` resource use the configured Lab scope; Gateway-managed protected MCP routes may advertise and enforce route-specific scopes such as `mcp:read mcp:write`.
3. **401** — if both checks fail (or neither auth method is configured for the token presented).

Static bearer tokens bypass all JWT validation. This allows operators to use a simple token for automation while also supporting OAuth for interactive or multi-tenant use.

For node runtime background traffic, the supported auth path in this implementation is the shared static bearer token when `LABBY_MCP_HTTP_TOKEN` is configured.

## Safety Gate

Lab refuses to bind on a non-localhost address without any auth configured:

```text
refusing to bind HTTP on 0.0.0.0:8765 without authentication.
Set LABBY_MCP_HTTP_TOKEN or LABBY_AUTH_MODE=oauth, or bind to 127.0.0.1 for local-only access.
```

Loopback hosts exempt from this check: `127.0.0.1`, `::1`, `[::1]`, `localhost`.

## Example: Deploying with OAuth

```bash
# In ~/.labby/.env
LABBY_MCP_TRANSPORT=http
LABBY_MCP_HTTP_HOST=0.0.0.0
LABBY_MCP_HTTP_PORT=8765
LABBY_AUTH_MODE=oauth
LABBY_PUBLIC_URL=https://lab.example.com
LABBY_GOOGLE_CLIENT_ID=google-client-id
LABBY_GOOGLE_CLIENT_SECRET=google-client-secret

# Start
labby serve
```

Verify the metadata endpoint:

```bash
curl https://lab.example.com/.well-known/oauth-protected-resource
```

Call a protected endpoint with a `lab` access token:

```bash
curl -H "Authorization: Bearer eyJhbG..." \
     https://lab.example.com/v1/marketplace \
     -d '{"action":"help"}'
```

## Verifying Auth Configuration

Two complementary verification surfaces exist:

### External probe — `scripts/check-oauth.sh`

An operator shell script that tests a **running server** from outside, using only `curl`. Useful after deploy, in CI pipelines, or from a remote machine.

```bash
# Auto-loads ~/.labby/.env; defaults to http://localhost:8080
./scripts/check-oauth.sh

# Point at a specific server
./scripts/check-oauth.sh https://lab.example.com

# Or via env var
LABBY_BASE_URL=https://lab.example.com ./scripts/check-oauth.sh
```

The script covers:

- Config presence (`LABBY_MCP_HTTP_TOKEN`, `LABBY_PUBLIC_URL`, Google credentials, `LABBY_WEB_UI_AUTH_DISABLED`)
- Health probes reachable without auth (`/health`, `/ready`)
- Protected endpoints return `401 {kind:auth_failed}` when unauthenticated (`/v1/*`, `/mcp`, `/v0.1/servers`)
- Static bearer token accepted and wrong tokens rejected
- MCP endpoint is bearer-only (session cookies rejected)
- OAuth discovery endpoints are public and structurally valid (`/.well-known/oauth-authorization-server`, `/.well-known/oauth-protected-resource`, `/jwks`)
- Issuer in `/.well-known/oauth-authorization-server` matches `LABBY_PUBLIC_URL`
- `WWW-Authenticate: Bearer resource_metadata=...` header present on 401 (RFC 9728)
- Dev marketplace endpoint is unauthenticated for reads, blocked for mutations
- Node self-registration endpoints are public
- Upstream OAuth browser callback is not behind bearer auth

Exit codes: `0` = all pass, `1` = one or more failures.

### Internal pre-flight — `labby doctor`

`labby doctor` is the in-process health audit. It checks config validity, file permissions on `auth.db` and `auth-jwt.pem`, service reachability, and auth configuration before you have a running server to probe. Use the shell script for post-deploy black-box verification; use `labby doctor` for pre-flight and service-level health.

Auth-specific items `labby doctor` covers (or should cover):

- `LABBY_PUBLIC_URL` is set when OAuth mode is active
- Google credentials present
- `auth.db` and `auth-jwt.pem` exist and have restrictive permissions (`0600`)
- SQLite store is openable (WAL mode, non-zero busy timeout)
- Signing key is loadable

## Related Docs

- [CONFIG.md](./CONFIG.md) — config loading and env var conventions
- [TRANSPORT.md](./TRANSPORT.md) — HTTP transport setup and middleware
- [ERRORS.md](../dev/ERRORS.md) — `auth_failed` error kind
- [RMCP.md](../surfaces/RMCP.md) — RMCP auth ownership contract
