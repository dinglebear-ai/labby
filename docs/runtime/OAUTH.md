---
title: "HTTP Auth Modes"
created: "2026-07-30"
updated: "2026-09-05"
---

# HTTP Auth Modes

Labby supports two HTTP auth modes:

- `LABBY_AUTH_MODE=bearer`
  Preserve the existing static bearer-token flow with `LABBY_MCP_HTTP_TOKEN`.
- `LABBY_AUTH_MODE=oauth`
  Run Labby's authorization server with exactly one inbound human identity
  provider: Google (stable) or Authelia OpenID Connect (open beta). Labby
  issues its own JWT access tokens and exposes JWKS plus RFC 9728 metadata.

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
| `LABBY_TOKEN_ENCRYPTION_KEY` | oauth mode | 32-byte key encoded as 64 hex digits or 43 base64url characters; encrypts reusable Google provider credentials and local refresh replay responses in `auth.db`. |
| `LABBY_PUBLIC_URL` | oauth mode | Public base URL for metadata and JWT issuer/audience. It also supplies the Google callback base unless `LABBY_GOOGLE_CALLBACK_URL` is set. Path-prefixed deployments are supported. |
| `LABBY_GOOGLE_CLIENT_ID` | Google provider | Google OAuth client ID. |
| `LABBY_GOOGLE_CLIENT_SECRET` | Google provider | Google OAuth client secret. |
| `LABBY_AUTH_PROVIDER` | no | Select exactly one inbound provider: `google` or `authelia`. Legacy Google-only configuration remains valid. |
| `LABBY_AUTHELIA_ISSUER_URL` | Authelia | Exact OIDC issuer URL. Discovery, token, and JWKS endpoints must remain on this origin. |
| `LABBY_AUTHELIA_CLIENT_ID` | Authelia | Confidential OIDC client ID. |
| `LABBY_AUTHELIA_CLIENT_SECRET` | Authelia | Confidential OIDC client secret; Labby uses `client_secret_basic`. |
| `LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN` | no | Explicitly trusts one exact HTTPS private issuer origin. It does not disable SSRF protection for any other origin. |
| `LABBY_AUTHELIA_CA_CERT_PATH` | no | Optional PEM CA certificate for the exact Authelia origin. Mount it read-only; this does not extend trust to other outbound requests. |
| `LABBY_AUTH_SQLITE_PATH` | no | Override path for the SQLite auth database. |
| `LABBY_AUTH_KEY_PATH` | no | Override path for the persisted JWT signing key. |
| `LABBY_AUTH_ALLOWED_REDIRECT_URIS` | no | Comma-separated redirect URI patterns allowed for dynamic client registration. When unset, Labby seeds common ChatGPT/Claude callback patterns. Set it explicitly to replace those defaults; use `https://*` only when the operator intentionally trusts any HTTPS DCR callback. Loopback/native-app callbacks are accepted by the auth layer. |
| `LABBY_AUTH_ADMIN_EMAIL` | oauth mode | Verified email address of the bootstrap admin for the selected provider. Normalized to lowercase at startup; startup fails closed if unset. Additional users come from the SQLite-backed allowlist. |
| `LABBY_AUTH_ALLOWED_EMAIL_DOMAINS` | no | Comma-separated domains whose members may log in, in addition to `LABBY_AUTH_ADMIN_EMAIL` and the SQLite-backed allowlist. Entries are trimmed, stripped of a leading `@`, and lowercased. Google authorization matches the provider-asserted `hd` claim; Authelia authorization matches the exact domain of its verified email claim. `email_verified` is enforced first. Empty (the default) disables domain-based access. |
| `LABBY_GOOGLE_CALLBACK_URL` | no | Absolute Google OAuth callback URL. Use this when the browser webapp host differs from the OAuth issuer; when unset, Labby derives the callback from `LABBY_PUBLIC_URL` and `LABBY_GOOGLE_CALLBACK_PATH`. |
| `LABBY_GOOGLE_CALLBACK_PATH` | no | Callback path appended to `LABBY_PUBLIC_URL`. Defaults to `/auth/google/callback`. |
| `LABBY_GOOGLE_SCOPES` | no | Comma-separated Google scopes. Defaults to `openid,email,profile`. |
| `LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `POST /register`. Defaults to `20`. |
| `LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `/authorize` and browser login initiation. Defaults to `60`. |
| `LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE` | no | Per-IP rate limit for credential-bearing `/token` and `/revoke` requests. Defaults to `120`. |
| `LABBY_AUTH_MACHINE_CLIENTS_JSON` | client credentials | JSON array of preregistered machine clients. Each entry selects exactly one of `client_secret` or `jwks` and lists allowed `resources` and `scopes`. |
| `LABBY_AUTH_ENTERPRISE_ISSUERS_JSON` | enterprise authorization | JSON array of trusted ID-JAG issuers with inline `jwks` or HTTPS `jwks_uri` and optional `allowed_client_ids`. |
| `LABBY_AUTH_MAX_PENDING_OAUTH_STATES` | no | Maximum non-expired pending authorization + browser-login states stored at once. Defaults to `1024`. |
| `LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY` | no | Explicit temporary workaround for [openai/codex#34684](https://github.com/openai/codex/issues/34684). When `true`, Labby neither advertises nor emits the RFC 9207 authorization-response `iss` parameter. Defaults to `false`; remove the override after the affected Codex callback handling is fixed. |

### Authelia open beta

Labby's pinned interoperability gate tests `authelia/authelia:4.39.10`. Register
one confidential authorization-code client in Authelia with:

- redirect URI `https://<exact LABBY_PUBLIC_URL authority>/auth/oidc/callback`;
- token endpoint authentication method `client_secret_basic`;
- `require_pkce: true` with only `S256`;
- scopes `openid`, `email`, and `profile`, with claims `sub`, `email`, and
  `email_verified`; and
- no `offline_access` grant. Labby intentionally stores no Authelia refresh token.

Authelia 4.39.10 requires the requested identity claims to be linked to the
client through a claims policy. A minimal client fragment is:

```yaml
identity_providers:
  oidc:
    claims_policies:
      labby:
        id_token: [email, email_verified]
    clients:
      - client_id: labby
        client_secret: '$pbkdf2-sha512$...'
        claims_policy: labby
        redirect_uris:
          - https://lab.example.com/auth/oidc/callback
        scopes: [openid, profile, email]
        grant_types: [authorization_code]
        response_types: [code]
        token_endpoint_auth_method: client_secret_basic
        require_pkce: true
        pkce_challenge_method: S256
```

Without the `claims_policy` link, authentication can succeed at Authelia but
Labby will reject the ID token because its verified email evidence is absent.

The callback is fixed for this provider. Google keeps its separate
`/auth/google/callback` contract; provider selection does not alias or leave
both callbacks active. Discovery runs at startup and requires exact issuer
equality. Authorization, token, and JWKS endpoints must stay on the issuer
origin, and redirects are rejected. Private-address issuers require
`LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN` to equal the exact HTTPS origin; a
private CA additionally requires `LABBY_AUTHELIA_CA_CERT_PATH`. Neither option
is a general SSRF or TLS bypass.

Changing provider kind, issuer, client ID, or fixed callback creates a new
provider generation. Restart every Labby process against the same configuration
and database; stale processes fail closed at callback/token publication. Trust
origin, CA path, and policy changes also require an all-process restart even
when the identity generation is unchanged. To roll back, restore the previous
complete provider configuration and restart all processes. Users authenticate
again after a provider-generation switch; identity state is never silently
reused across generations.

Authelia renewal is local-policy-only: Labby rotates its own refresh token
without contacting the IdP. Removing a local allowed user/domain or changing
the provider generation invalidates sessions and renewable grants immediately.
Authelia-side disablement is observed only at the next interactive login or
when existing Labby session/refresh lifetimes end. An already-issued stateless
Labby access JWT remains usable for at most `LABBY_AUTH_ACCESS_TOKEN_TTL_SECS`;
choose that TTL to match required offboarding latency.

RP-initiated logout is not supported. If the Labby signing key is compromised,
stop all issuers, replace `LABBY_AUTH_KEY_PATH`, restart every process, and
invalidate active sessions/refresh grants; signing-key rotation is the
emergency way to invalidate residual access JWTs. If the Authelia client secret
or private CA material is compromised, rotate it at Authelia, replace the
server-held secret/file, and restart all Labby processes. Never put secrets in
TOML, command lines, logs, or support bundles.
## Startup Behavior

When OAuth mode is configured, `labby serve` performs these steps at startup:

1. Validate `LABBY_PUBLIC_URL`, `LABBY_AUTH_ADMIN_EMAIL`, and exactly one
   complete selected provider configuration. Both providers require a valid
   `LABBY_TOKEN_ENCRYPTION_KEY`; Authelia additionally requires its issuer and
   any explicitly configured private-origin/CA trust material.
2. Open the SQLite auth store in WAL mode with a non-zero busy timeout. Legacy
   plaintext Google provider rows are atomically encrypted before they can be
   served.
3. Load or generate the persisted Ed25519 signing key. Legacy RSA key files are
   quarantined and rotated on first startup after upgrade.
4. Construct only the selected provider. Explicit Google uses its fixed Google
   callback; Authelia performs bounded discovery and uses the fixed
   `/auth/oidc/callback`.

Startup fails closed if any of those steps fail.

Startup also fails if:

- `LABBY_AUTH_MODE=oauth` is set without `LABBY_PUBLIC_URL`
- the selected provider configuration is incomplete or both providers are ambiguous
- OAuth is selected without `LABBY_TOKEN_ENCRYPTION_KEY`
- `LABBY_AUTH_ADMIN_EMAIL` is missing, so no provider identity is explicitly permitted
- the auth database or signing key has insecure file permissions

## Owner bootstrap and project-bound credentials

Labby supports two separate first-owner flows:

- `POST /v1/access/bootstrap-owner` remains the inbound-provider-backed browser flow. It
  requires the authenticated browser session, CSRF token, `lab:admin`, the
  canonical external identity, and an email matching
  `LABBY_AUTH_ADMIN_EMAIL`.
- `labby setup access-bootstrap` is the direct-local operator flow documented
  in [Local access bootstrap](../guides/LOCAL_ACCESS_BOOTSTRAP.md). Eligibility
  comes only from a one-time 256-bit proof prepared offline while the
  installation is pristine. Loopback location by itself grants nothing.

The local flow binds the first owner, default Project, already-published
Loadout, protected route, resource, audience, ordered scopes, installation
generation, and client-generated credential digest in one access-store
transaction. It does not depend on OAuth being configured. Its consume,
status, and cleanup routes accept `X-Labby-Bootstrap-Proof` only from a direct
loopback or Unix peer, reject forwarded authority, return uniform
non-enumerating denials, and set private no-store/no-referrer response headers.

Issued `lby_pc_v1_...` product credentials are distinct from OAuth access
tokens and from `LABBY_MCP_HTTP_TOKEN`. Every request revalidates the source
credential, project membership, policy epochs, published Loadout/route
generations, resource, audience, and scopes. Revocation therefore immediately
denies both the credential and browser sessions derived from it, even if
best-effort session-row cleanup has not completed.

`POST /token` continues to exchange OAuth grants only; it never consumes a
bootstrap proof or issues a project credential. Never edit the access database,
forge credential rows, or treat loopback reachability as bootstrap authority.

## Registration and Authorize Flow

OAuth mode exposes:

- `POST /register`
- `GET /authorize`
- `GET /auth/google/callback`
- `GET /auth/oidc/callback`
- `GET /native/callback`
- `POST /native/poll`
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

Clients may skip `POST /register` entirely and use a Client ID Metadata
Document (CIMD) — an HTTPS URL as the `client_id`, per
`draft-ietf-oauth-client-id-metadata-document`. Labby advertises this with
`client_id_metadata_document_supported: true`. Document rules:

- `client_id` in the document must exactly equal the URL it was fetched from
- `client_name` and at least one `redirect_uris` entry are required, and every
  redirect URI is held to the same allowlist as DCR registration
- `token_endpoint_auth_method` names the client's preferred method and must be
  `none` or `private_key_jwt`. A client that can authenticate more than one way
  may also publish `token_endpoint_auth_methods_supported`; Labby accepts any
  method in that set, and every listed method must also be one of the two.
  Omitting the field means the preference is the only accepted method. Note
  that `token_endpoint_auth_methods_supported` is an RFC 8414 *authorization
  server* metadata name; ChatGPT's connector publishes it in its **client**
  document, and Labby honours it there rather than rejecting a client for
  using a method it advertises
- a set containing `private_key_jwt` still requires keys, even when the
  declared preference is `none`
- a `private_key_jwt` document must publish its public keys, either inline as
  `jwks` or by reference as `jwks_uri` (the form ChatGPT's connector uses).
  Inline keys take precedence and suppress the `jwks_uri` fetch
- a `jwks_uri` must pass the shared SSRF preflight (HTTPS, no private,
  loopback, link-local, CGNAT, or private-TLD host); an unusable one is
  rejected at `/authorize` rather than deferred to `/token`
- the referenced key set is fetched lazily at client-assertion validation and
  cached by URL; a `kid` that is absent from the cached set forces a refetch,
  so client key rotation does not have to wait out the cache TTL

Flow summary:

1. A client registers a loopback redirect URI, a native-app URI, a product-default callback URI, or one that matches the configured allowlist.
2. The client sends the user to `/authorize` with `response_type=code`.
3. Labby stores the request state, generates PKCE data, and redirects to the
   selected provider.
4. The selected provider redirects back to its exclusive callback:
   `/auth/google/callback` for Google or `/auth/oidc/callback` for Authelia.
5. Labby enforces the merged allowlist: `LABBY_AUTH_ADMIN_EMAIL`, configured
   Workspace domains, and the current SQLite-backed allowed-user list managed
   through settings. The id_token's `email_verified` claim is required —
   unverified accounts are rejected even when an address or domain matches.
   Browser-login callers receive a 401; OAuth-client callers receive an RFC
   6749 §4.1.2.1 redirect with `error=access_denied`.
6. Labby exchanges the provider code server-side, stores a local authorization
   code, and redirects the client back to its registered redirect URI with the
   local code.
7. The client exchanges that local code at `/token` for a Labby access token.
   Google may also yield a renewable Labby grant when it granted upstream
   offline access; Authelia renewal is local-policy-only.

Google access and refresh tokens remain server-side only.

### Server-hosted native callback polling

Native clients that register Labby's advertised `native_callback_endpoint`
start authorization by requesting `/authorize` with
`Accept: application/vnd.labby.native-oauth-start+json`. Labby returns a
no-store JSON object containing `authorization_url` and an independent,
high-entropy `poll_token`. The client opens `authorization_url`, then sends
`POST native_poll_endpoint_v2` with JSON `{ "poll_token": "..." }` until it receives the one-shot local
authorization code.

The caller-provided OAuth `state` remains CSRF and callback correlation only;
it is never accepted as a polling credential. Labby stores only a SHA-256 hash
of `poll_token`, binds that hash to the pending client/redirect/PKCE request,
and transfers it to the one-shot result only after the provider callback has
redeemed the matching server state. A request that knows `state` but not the
server-generated `poll_token` receives `202 Accepted` and cannot retrieve the
code.

The polling credential never appears in a URI or redirect and therefore does
not enter access logs, browser history, proxy request targets, or Referer
headers. Both start and poll responses use `Cache-Control: no-store`.

This contract is advertised only through `native_poll_endpoint_v2` plus
`native_authorization_start_media_type`. The legacy `native_poll_endpoint`
field is deliberately absent. Older Palette releases consequently fall back
to their loopback OAuth flow instead of attempting state-keyed polling. Deploy
the Labby server first; upgraded Palette clients then discover v2
automatically. Old and new clients remain safe during a rolling upgrade.

Google-specific notes:

- Labby sends `access_type=offline` when redirecting to Google so the provider can issue a refresh token
- Labby also sends `prompt=consent` so a fresh Google consent flow can return a new refresh token after the app was previously authorized without offline access
- if Google still does not return an upstream refresh token, Labby omits `refresh_token` from its token response and later refresh grants fail closed
- Labby validates the Google `id_token` cryptographically against Google JWKS and rejects tokens with the wrong issuer, audience, or expiry before minting any local identity

## Browser-Local Callback Forwarding

Labby also ships a local OAuth callback forwarder for browser-side machines:

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
https://callback.example.com/callback/<machine>
https://callback.example.com/callback/<machine>/<suffix>
```

This is for remote, headless, or cross-namespace clients whose browser cannot
reach the client's local loopback listener directly. Regular non-headless
desktop clients should keep local loopback callbacks where possible.

Client configuration example:

```toml
mcp_oauth_callback_url = "https://callback.example.com/callback/devhost"
```

The public relay is transport-only. It forwards the final callback request to
the registered machine target; Codex or the MCP client still owns PKCE, state
validation, and token exchange.

Public relay constraints:

- public callback routes are unauthenticated: `GET|POST /callback/<machine>[/*suffix]`
- admin mutation lives under authenticated `/v1/oauth/relay/*` and requires `lab:admin`
- targets must be `http://<tailscale-ip>:38935/callback/<machine>` (host in the Tailscale CGNAT range `100.64.0.0/10`, e.g. `http://100.99.0.1:38935/callback/devhost`) with no userinfo, query, or fragment
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
  --machine devhost \
  --target-url http://100.99.0.1:38935/callback/devhost
labby oauth relay-registry disable --machine devhost
labby oauth relay-registry enable --machine devhost
labby oauth relay-registry remove --machine devhost
```

CLI registry mutations write the sidecar file and report
`restart_required: true`; restart `labby serve` to refresh a running server's
in-memory snapshot. The authenticated admin API updates the live snapshot and
the sidecar together. Registry imports are all-or-nothing: any quarantined
machine or invalid target rejects the import without replacing the active
registry.

For the production cutover and rollback procedure, see
[CALLBACK_RELAY.md](./CALLBACK_RELAY.md).

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
https://callback.example.com/callback/devhost
  -> http://100.99.0.1:38935/callback/devhost
```

For Linux sessions without a usable desktop keyring or D-Bus session, prefer
file-backed MCP OAuth credentials:

```toml
mcp_oauth_credentials_store = "file"
```

This is mainly a headless/SSH workaround. Do not force it for ordinary desktop
clients where the platform credential store works.

### Using non-loopback redirect URIs

Loopback redirect URIs are always accepted by `labby-auth`. Native-app private-use
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

Labby advertises and enforces RFC 9207 authorization-response issuer binding, so
current ChatGPT custom MCP connectors should select the stable redirect
`https://chatgpt.com/connector_platform_oauth_redirect` together with the stable
CIMD client ID `https://chatgpt.com/oauth/client.json`. The callback-ID form
`https://chatgpt.com/connector/oauth/{callback_id}` is ChatGPT's fallback for an
authorization server that does not meet those issuer-identification
requirements; it remains allowlisted for compatibility, not as the preferred
Labby path. Labby's product defaults cover both forms plus the legacy
`https://chatgpt.com/aip/plugin-callback`, and Claude's
`https://claude.ai/api/mcp/auth_callback`. These are redirect allowlist entries,
not client identifiers: a ChatGPT CIMD client still uses its exact HTTPS
metadata-document URL as `client_id`, and Labby validates the document as
described above. Treat the stable CIMD URL as the client ID, not as a redirect
URI or an authorization-server metadata location. Other
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

### Supported Algorithms

- Labby-issued access tokens: EdDSA (Ed25519)
- Google ID tokens: RS256 verification only

### Scopes

Current Labby tokens use the standard space-delimited `scope` claim.

### AuthContext

On successful validation, an `AuthContext` is injected into the request extensions:

- `sub` — the authenticated user/client identifier from the `sub` claim.
- `scopes` — granted scopes.
- `issuer` — token issuer.

Downstream handlers can read `AuthContext` from request extensions for audit trails and scope-gated access.

Signature-valid access tokens minted before canonical identity provenance was
introduced still receive `AuthContext` for compatibility with ordinary
authenticated routes. They do not receive a `VerifiedIdentity` extension, so
project/access boundaries and other identity-gated handlers fail closed.
Malformed, incomplete, or conflicting provenance remains an authentication
failure rather than falling back to this legacy-token behavior.

## Token Exchange

`POST /token` supports:

- `grant_type=authorization_code`
- `grant_type=refresh_token`
- `grant_type=client_credentials` when a machine client is configured
- `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer` when an enterprise
  issuer is configured

Current constraints:

- authorization-code redemption is atomic and single-use
- `refresh_token` is only issued when Google returned an upstream refresh token
- refresh grants are rejected if the local token is not backed by an upstream refresh token
- successful refresh grants atomically rotate the local refresh token; the old
  token is invalid immediately
- `POST /revoke` implements idempotent refresh-token revocation
- machine clients are preregistered out of band with
  `LABBY_AUTH_MACHINE_CLIENTS_JSON` and authenticate with `client_secret_basic`
  or RFC 7523 `private_key_jwt`
- trusted enterprise ID-JAG issuers are configured with
  `LABBY_AUTH_ENTERPRISE_ISSUERS_JSON`; assertions must use
  `typ=oauth-id-jag+jwt` and are issuer, audience, client, resource, scope,
  expiry, signature, and replay validated
- successful and failed `/token` responses must send `Cache-Control: no-store`
  and `Pragma: no-cache`

Example machine-client configuration:

```json
[
  {
    "client_id": "ci-agent",
    "client_secret": "load-this-from-a-secret-manager",
    "scopes": ["lab"],
    "resources": ["https://lab.example.com/mcp"]
  }
]
```

For `private_key_jwt`, omit `client_secret` and provide a standard `jwks`
document — machine clients are preregistered, so they carry inline keys only,
unlike CIMD clients which may also publish `jwks_uri`. Enterprise issuer
entries use `issuer`, either `jwks_uri` (HTTPS) or
an inline `jwks`, and an optional `allowed_client_ids` list. Remote CIMD and
JWKS reads reject redirects, non-HTTPS URLs, private/loopback DNS results, and
oversized CIMD responses; successful documents are cached according to
`Cache-Control: max-age`.

### Auth Failure Semantics

Labby distinguishes unauthenticated callers from internal auth outages.

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

### Access owner bootstrap

`POST /v1/access/bootstrap-owner` is mounted only when OAuth browser state is
configured and is stricter than ordinary `/v1` routes. It accepts only an OAuth browser session with a matching `X-CSRF-Token`,
middleware-derived canonical `VerifiedIdentity`, `lab:admin`, and an
authenticated email equal to `LABBY_AUTH_ADMIN_EMAIL`. The email is the
eligibility gate for this initial operation; the durable Principal link uses
the verified provider issuer and subject.

Bearer authentication, static/local credentials, MCP, CLI, stdio, and loopback
origin do not substitute for those requirements. Success returns only
`{"status":"created"}` or `{"status":"already_applied"}` with
`Cache-Control: private, no-store`; handler failures use the canonical agent error
envelope, while authentication and CSRF failures retain the shared auth-middleware
envelope. Without OAuth browser state, the route is absent and returns `404`
before body validation. See [Access Owner Bootstrap](../services/ACCESS.md).

The access-control database is separate from the OAuth authorization store. It
is fixed at the absolute path `$LABBY_HOME/access.db` (default
`~/.labby/access.db`); `LABBY_AUTH_SQLITE_PATH` does not relocate it. A
gateway-subset protected route opts into Project authorization context with
`target.project_id`. Route add/test accept `--project-id`; update preserves the
current binding unless `--project-id` replaces it or `--clear-project-id`
explicitly removes it.

Allowlist removal is an immediate revocation boundary for renewable browser
and upstream credentials. `DELETE /v1/auth/allowed-emails/{email}` resolves
every subject associated with the email, then atomically removes the allowlist
entry, browser sessions, local refresh grants, pending authorization codes, and
central Google provider credentials while advancing the provider revocation
epochs. Before the request succeeds, Labby also evicts the subjects from its
OAuth client cache and drains their generic, subject-scoped, relay, and
task-retained upstream peers. A later upstream use must therefore authorize
again instead of reusing an old credential or connection.

Already-issued signed access tokens are stateless and therefore remain usable
only until their configured `LABBY_AUTH_ACCESS_TOKEN_TTL_SECS` expiry (3600
seconds by default); removal prevents them from being renewed. This bounded
residual lifetime is the only intentionally non-immediate part of allowlist
revocation.

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
[Errors and recovery](../dev/ERRORS.md).

Documented auth-specific exception:

- `invalid_grant` remains a stable OAuth token/authorization error for
  authorization-code and refresh-token redemption failures such as expired,
  unknown, or mismatched grants
- `oauth_needs_reauth` is returned with `401 Unauthorized` when Google rejects
  the provider refresh token with `invalid_grant`; the client must reconnect to
  complete a new interactive authorization flow

## RFC 9728 Protected Resource Metadata

Labby exposes a metadata endpoint so MCP clients can discover which authorization server to use:

```http
GET /.well-known/oauth-protected-resource
```

This endpoint is **unauthenticated** — clients need it before they have a token.

Response:

```json
{
  "resource": "https://lab.example.com",
  "authorization_servers": ["https://lab.example.com"],
  "scopes_supported": ["lab:read", "lab", "lab:admin"],
  "bearer_methods_supported": ["header"]
}
```

### WWW-Authenticate Header

When a request fails authentication (401), the response includes:

```http
WWW-Authenticate: Bearer resource_metadata="https://lab.example.com/.well-known/oauth-protected-resource", scope="lab:read"
```

This header is only included when `LABBY_PUBLIC_URL` is configured. If not, the
header is omitted rather than advertising localhost. Challenges for read-only
discovery use `scope="lab:read"`; execution and administrative operations may
step up to `lab` or `lab:admin` without changing the protected resource.

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
Labby admin/API surfaces; Gateway-managed public MCP routes validate Labby OAuth
JWTs whose audience is the route resource.

Disabled or unknown protected routes must not advertise protected-resource
metadata or proxy to a backend. They should fail with a stable public error that
does not leak backend origins, backend paths, private IPs, or token env var
names.

Full route configuration and curl verification examples live in
[GATEWAY.md](../services/GATEWAY.md#gateway-managed-protected-mcp-routes).

## Direct Stdio Proxy OAuth

`labby proxy --auth oauth` uses the same stable Labby authorization-server
issuer but creates an ephemeral protected resource for the exact printed proxy
URL. The external port and MCP path are both part of the JWT audience:

```text
https://node.example.ts.net:53147/mcp
```

A random-port run is therefore a new resource on every invocation. Prefer a
fixed proxy port for a long-lived connector configuration.

Before publication, the CLI verifies the live daemon's issuer metadata, JWKS,
and gateway action inventory, then uses authenticated `POST /v1/gateway` action
dispatch to create, renew, and release the resource lease. There are no
route-local or dedicated `/v1/internal/*` lease endpoints. Normal shutdown
releases the lease; a forced termination recovers through the 120-second TTL
and the daemon's periodic expiry sweep.

Unlike configured Gateway protected routes, the direct proxy serves metadata
at the origin root, with no route suffix:

```http
GET https://node.example.ts.net:53147/.well-known/oauth-protected-resource
```

The document's `resource` remains the exact `/mcp` URL above, and the
`WWW-Authenticate` challenge points to that root metadata document. A token for
the same host with another port or path is rejected. Local HTTP OAuth is not
supported because resource leases accept HTTPS audiences; startup fails rather
than downgrading to bearer or none.

See the [stdio MCP proxy guide](../guides/STDIO_MCP_PROXY.md#oauth-resource-lifecycle)
for daemon discovery, lease timing, fixed-port guidance, and troubleshooting.

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
project_id = "project-42"
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

In Code Mode visibility, ChatGPT sees the small Labby-owned synthetic surface
instead of one action per upstream tool. `codemode_read` accepts `lab:read`,
`lab`, or `lab:admin` and can invoke only tools whose live descriptor explicitly
sets `readOnlyHint: true` without a contradictory `destructiveHint: true`.
`codemode` and the optional `codemode_ui` require `lab` or `lab:admin` and retain
full execution authority. On the root gateway, the always-available `mcp_app`
control tool uses the same read/open scopes, while changing Labby-owned app
visibility requires `lab:admin`. Its own manager UI is opt-in like every other
Labby-owned app surface. The control tool is omitted from protected subset routes
so a subset-scoped token cannot mutate gateway-global UI visibility.

Gateway management actions on a protected `gateway_subset` route are bounded
to that route's configured upstream allowlist. Every aggregate listing —
`gateway.list`, `gateway.status`, `gateway.mcp.list`, `gateway.import_pending.list`,
`gateway.import_tombstones.list`, and `gateway.skills.list` — contains only
route-visible upstreams. Every operation that names a single upstream rejects
one outside the subset as unknown: configuration, status, discovery,
client-config, test, update, and remove, plus the MCP lifecycle operations
(`enable`, `disable`, `restart`, `cleanup`), the `gateway.oauth.*` family, and
the import and tombstone operations. A subset route additionally refuses two
operation rather than scoping it: `gateway.test` with an unsaved inline `spec`,
which would otherwise execute an arbitrary stdio command outside the mounted
subset. These restrictions apply even when the token has an admin scope; the
route remains an authority boundary, not merely a catalog filter.

Creation is the deliberate exception. `gateway.add` and
`gateway.import_pending.approve` are not bounded by the route's allowlist — the
upstream they create does not exist yet, so there is nothing to check a name
against — and they may create an upstream the route does not expose. Alongside
them the gateway-global mutation surface — `gateway.reload`,
`gateway.protected_route.*`,
`gateway.loadout.*`, `gateway.virtual_server.*`, `gateway.service_config.set`,
`gateway.code_mode.set`, `gateway.discover`, and `gateway.import` with
`all: true` — is gated by scope alone, not by the route's upstream allowlist. A
subset route whose token carries an admin scope can still reach those. Do not
issue admin-scoped tokens for subset routes and treat the allowlist as the only
boundary.

Synthetic Code Mode keeps ordinary raw upstream tools out of the approval-facing
catalog. Upstream MCP App owners and callbacks pass through only when the same
allowed upstream exposes a real native `ui://` app binding and proxies that
resource; both `proxy_resources` and `expose_resources` are enforced. Callback
metadata alone does not escape raw-tool suppression, ambiguous names fail
closed, and destructive app tools require `lab` or `lab:admin` rather than
`lab:read`.

For OAuth upstreams, the app tool catalog is taken only from that caller's cached
subject connection. Native `ui://` reads resolve back to that same subject-bound
upstream and preserve relay/cancellation metadata; a subject-scoped resource
policy denial cannot fall through to a global connection. Other upstream churn
remains discoverable inside `codemode.search(...)` / `codemode.describe(...)`
without expanding the host Tool JSON.

## Auth Precedence

When both static bearer and OAuth are configured, auth is checked in this order:

1. **Static bearer token** — constant-time comparison via `LABBY_MCP_HTTP_TOKEN`. If it matches, the request is authenticated with implicit `lab:read` and `lab:admin` scopes.
2. **OAuth JWT** — if the static bearer check fails (or no static token is configured), the token is validated as a JWT against the cached JWKS. Tokens for Labby's own `/mcp` resource use the configured Labby scope; Gateway-managed protected MCP routes may advertise and enforce route-specific scopes such as `mcp:read mcp:write`.
3. **401** — if both checks fail (or neither auth method is configured for the token presented).

Static bearer tokens bypass all JWT validation. This allows operators to use a simple token for automation while also supporting OAuth for interactive or multi-tenant use.

For node runtime background traffic, the supported auth path in this implementation is the shared static bearer token when `LABBY_MCP_HTTP_TOKEN` is configured.

## Safety Gate

Labby refuses to bind on a non-localhost address without any auth configured:

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
# Optional when the webapp and OAuth issuer use different hosts:
# LABBY_GOOGLE_CALLBACK_URL=https://labby.example.com/auth/google/callback
LABBY_GOOGLE_CLIENT_ID=google-client-id
LABBY_GOOGLE_CLIENT_SECRET=google-client-secret
# Generate and persist this secret as described below; do not use this placeholder.
LABBY_TOKEN_ENCRYPTION_KEY=<64-lowercase-hex-digits>

# Start
labby serve
```

Verify the metadata endpoint:

```bash
curl https://lab.example.com/.well-known/oauth-protected-resource
```

Call a protected endpoint with a Labby access token:

```bash
curl -H "Authorization: Bearer eyJhbG..." \
     https://lab.example.com/v1/gateway \
     -H "Content-Type: application/json" \
     -d '{"action":"gateway.list","params":{}}'
```

## Verifying Auth Configuration

### Credential-encryption key lifecycle

Generate the key from the operating-system CSPRNG and write it directly to the
owner-only environment file; never paste it into logs, tickets, or command
output. A compatible value is 32 random bytes encoded as lowercase hex:

```bash
umask 077
key="$(openssl rand -hex 32)"
# Merge LABBY_TOKEN_ENCRYPTION_KEY=$key into /home/labby/.labby/.env using
# `labby setup` or another backup-first atomic secret-file editor.
unset key
```

`labby setup --provision`, host-service install, and host-service restart
preflight an existing `/home/labby/.labby/.env`. When OAuth is active and the
key is missing, setup creates a timestamped `.env.bak.*` first, atomically
merges a generated key with mode `0600`, restores `labby:labby` ownership, and
only then restarts the service. Existing valid keys are preserved byte for byte.
An invalid configured key aborts the restart rather than replacing it.

Provider cutover is a stop-the-world maintenance operation. Stop every Labby
process that can write the shared auth database, run `labby doctor auth --live`
with the proposed Authelia configuration, and take a SQLite backup before the
first new binary starts. Provider discovery completes before the v15 migration,
but once v15 commits, older writers must remain stopped. Verify the durable state
after startup with:

```sql
PRAGMA user_version;
PRAGMA integrity_check;
PRAGMA foreign_key_check;
SELECT COUNT(*) AS provider_rows FROM inbound_identity_provider;
SELECT provider, issuer, generation FROM inbound_identity_provider WHERE singleton = 1;
SELECT 'authorization_codes' AS source, identity_issuer, provider_generation, COUNT(*)
FROM authorization_codes GROUP BY identity_issuer, provider_generation
UNION ALL
SELECT 'refresh_tokens', identity_issuer, provider_generation, COUNT(*)
FROM refresh_tokens GROUP BY identity_issuer, provider_generation
UNION ALL
SELECT 'browser_sessions', identity_issuer, provider_generation, COUNT(*)
FROM browser_sessions GROUP BY identity_issuer, provider_generation;
```

The expected final `user_version` is `16`: provider metadata and identity
backfill are installed by v15, then v16 adds expiry-leading cleanup indexes.
`integrity_check` must return `ok`,
`foreign_key_check` must return no rows, and the singleton provider query must
return exactly one row matching the selected provider. Compare each grouped
identity-bearing row count with its pre-migration table count; v14 rows must be
mapped to Google's canonical issuer and generation `1`.

Rollback after the v15/v16 migration sequence requires stopping all writers and restoring the matching
pre-cutover database backup; changing only the binary or provider environment
is not a supported downgrade.

Back up the following as one recovery set before upgrades and before changing
OAuth configuration:

- the effective `.env`, including `LABBY_TOKEN_ENCRYPTION_KEY`;
- `auth.db` via SQLite's backup API or while the service is stopped;
- the JWT signing key at `LABBY_AUTH_KEY_PATH`.

Store the recovery set in an encrypted secret backup with access controls at
least as strict as the production environment. A database copy without the
matching token-encryption key cannot decrypt provider credentials. Loss of the
key is intentionally unrecoverable: stop the service, restore a matching
database/key backup or revoke the affected Google grants and rebuild the auth
database, then require users to authorize again. Do not generate a replacement
over the existing encrypted database and expect old credentials to survive.

Key rotation is an offline migration, not an environment-only edit:

1. Stop or drain Labby OAuth writes.
2. Create and verify a consistent backup of the database, old key, environment,
   and JWT signing key.
3. Decrypt every encrypted provider credential with the old key and re-encrypt
   it with the new key in one transaction using a supported migration tool.
4. Atomically update the environment to the new key, restart, run `labby doctor`,
   and perform a read-only OAuth smoke test.
5. Retain the old recovery set until the new database/key pair is verified.

There is currently no supported online key-rotation command. Never rotate by
editing only `LABBY_TOKEN_ENCRYPTION_KEY`; that makes existing ciphertext
undecryptable and OAuth startup/use will fail closed.

Current verification is owned by Labby's built-in health/doctor surfaces and focused integration tests; there is no checked-in `scripts/check-oauth.sh` product contract.

### Pre-flight — `labby doctor auth`

Use `labby doctor auth` to inspect auth/OAuth environment, persisted files, permissions, and configuration before or alongside a running server. Use `labby doctor proxy` for caller-visible public proxy checks and `labby doctor oauth-relay` for callback-relay registry/target checks.

### Running-server checks

A deployed server should be verified through its real public surface:

- `/health` and `/ready` reachability/readiness
- static bearer or OAuth authorization on protected `/v1/*` and `/mcp` routes
- OAuth authorization-server/protected-resource metadata and JWKS when OAuth mode is enabled
- issuer/resource audiences derived from the configured public URLs
- upstream OAuth callback/relay behavior when those features are configured

Use focused integration tests for exact protocol/status/header assertions; do not copy historical Marketplace, Fleet/node, or Registry-browser endpoint checks into current deployment runbooks.

## Related Docs

- [CONFIG.md](./CONFIG.md) — config loading and env var conventions
- [TRANSPORT.md](../surfaces/TRANSPORT.md) — HTTP transport setup and middleware
- [ERRORS.md](../dev/ERRORS.md) — `auth_failed` error kind
- [RMCP.md](../surfaces/RMCP.md) — RMCP auth ownership contract
