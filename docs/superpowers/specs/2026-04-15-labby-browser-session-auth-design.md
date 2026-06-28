# Labby Browser Session Auth Design

**Date:** 2026-04-15

**Related specs:**

- `2026-04-14-labby-single-binary-design.md`
- `2026-04-15-oauth-local-callback-relay-design.md`

## Goal

Add first-class browser authentication for the hosted Labby UI while preserving the static-asset deployment model from `lab serve`.

The target state is:

- the Labby web UI can authenticate real users without embedding bearer tokens into the frontend bundle
- `lab` remains the OAuth authorization server and the owner of upstream Google/GitHub identity flows
- the hosted browser UI uses a Rust-owned session cookie for `/v1/*`
- MCP clients continue using token-presented auth on `/mcp` via static bearer tokens or OAuth-issued `lab` JWT bearer tokens

## Context

The current hosted model has an architectural gap:

- `lab serve` can host the static Labby assets, HTTP API, OAuth endpoints, and MCP HTTP surface from one Rust process
- `/v1/*` and `/mcp` are protected by bearer/JWT auth middleware
- the web shell at `/` is public
- the exported Labby frontend does not have a real browser auth flow
- the current long-term-unacceptable workaround is to embed a bearer token into the frontend build via `NEXT_PUBLIC_API_TOKEN`
- there is also a temporary trusted-proxy escape hatch via `LAB_WEB_UI_DISABLE_AUTH=true` / `web.disable_auth = true`, but that intentionally weakens `/v1/*` auth and is not the real hosted-ui solution

That token-embedding path is acceptable for local smoke testing but not for a real hosted admin UI because anyone who can load the app can extract the token and reuse it against the API.

The static-export constraint is already a project decision. That rules out relying on live Next.js route handlers or a separate Node-hosted auth/session backend.

## Decision

Adopt a Rust-owned browser session model for the hosted Labby UI.

The browser will still start with the existing OAuth authorization flow owned by `lab`, but after successful login Rust will issue a browser session cookie scoped to the hosted Labby origin. The Labby UI will then call `/v1/*` using same-origin cookie auth rather than browser-held bearer tokens.

This is preferred over browser-held access/refresh tokens because it:

- keeps secrets and refresh state under Rust ownership
- avoids embedding bearer tokens into the static bundle
- fits the same-origin single-binary hosting model
- keeps the browser UI simple

## Non-Goals

This design does not include:

- changing MCP clients to use browser sessions
- removing bearer or JWT auth from the HTTP API
- moving Labby back to a live Next.js server
- introducing Tailscale-specific browser auth
- designing a multi-origin session-sharing model

`/mcp` remains an authenticated machine/client interface, not a browser session surface.

## High-Level Model

Two auth models will coexist intentionally:

### Browser UI auth

- used by the hosted Labby web UI
- established via the existing Rust-owned OAuth flow
- represented by an `HttpOnly` session cookie
- accepted on `/v1/*`

### Token-presented auth

- used by MCP clients, scripts, curl, and automation
- represented by static bearer tokens or OAuth-issued `lab` JWT access tokens presented as bearer tokens
- accepted on `/v1/*`
- required on `/mcp`

The browser UI and MCP server do not need to share the same auth transport. They serve different clients and should keep distinct operational contracts.

## Architecture

### System Boundary

`crates/lab-auth` becomes responsible not only for OAuth authorization-server behavior, but also for browser-session issuance and validation.

`crates/lab` remains responsible for:

- routing
- middleware wiring
- API surface ownership
- static Labby asset delivery

In the current codebase, the mounted auth surface is assembled in `crates/lab/src/api/router.rs`. `crates/lab-auth/src/routes.rs` exists, but it is not the runtime entrypoint for `lab serve`, so session/login routes must be wired through the `lab` HTTP router even if their core logic lives in `crates/lab-auth`.

`apps/gateway-admin` remains a static exported browser client. It does not hold tokens, run PKCE locally, or own the OAuth callback exchange.

### Browser Login Flow

1. An unauthenticated Labby user opens `https://lab.example.com/`.
2. The UI discovers it is not logged in by calling a lightweight session-status endpoint.
3. The UI redirects the browser to a Rust-owned browser login entrypoint such as `GET /auth/login`.
4. That browser login entrypoint starts the upstream Google/GitHub flow.
5. `lab` performs the upstream Google/GitHub flow exactly as it does today.
6. On successful completion, `lab` creates a browser session and sets a secure `HttpOnly` cookie on the Labby origin.
7. `lab` redirects the browser back to the hosted UI.
7. The UI re-checks session status and begins calling `/v1/*` as an authenticated browser client.

The browser never needs to store a `lab` access token or refresh token.

The browser-login flow should remain distinct from the existing OAuth client flow. Today, `GET /authorize` plus `GET /auth/google/callback` in `crates/lab-auth/src/authorize.rs` mints a local authorization code and redirects to a registered OAuth client callback. Reusing that path unchanged for the hosted UI would break the current OAuth client contract.

### Callback Paths

There are two callback concepts and they should not be conflated:

- the upstream Google callback currently terminates at `https://lab.example.com/auth/google/callback`
- the browser-facing post-login return target for the hosted UI can be `/`, `/app`, or a dedicated route such as `https://lab.example.com/auth/callback`

If a browser-facing route such as `/auth/callback` is introduced, it is a UI return target after the session cookie is already established. It is not the same thing as the upstream Google callback path.

Any design that changes the upstream callback path must explicitly include:

- `LAB_GOOGLE_CALLBACK_PATH` updates
- provider registration changes
- compatibility impact on the existing OAuth client flow

### Session Storage

Rust must own the browser session state. The session implementation may be backed by the existing auth database or a sibling store in the same Rust-owned auth subsystem, but the ownership contract is:

- session creation happens server-side
- session lookup happens server-side
- session revocation happens server-side
- the browser holds only the opaque session cookie

The session cookie should be:

- `HttpOnly`
- `Secure`
- `SameSite=Lax` at minimum
- path-scoped to the hosted app/API surface as appropriate

If the session cookie name is product-specific, it should be clearly distinct from generic OAuth artifacts.

## API Auth Contract

### `/v1/*`

The product API should accept either:

- a valid presented bearer token, including OAuth-issued `lab` JWT access tokens
- a valid browser session cookie

This allows:

- the hosted UI to operate with browser session auth
- scripts and automation to continue using bearer tokens

The API middleware should normalize both modes into a shared authenticated request context so handlers do not fork on transport.

### `/mcp`

`/mcp` should remain token-authenticated only.

Reasons:

- MCP clients are not browser-first
- browser session cookies are unnecessary for the current MCP use cases
- keeping `/mcp` token-based avoids muddying the MCP auth contract
- it reduces cross-surface complexity while still solving the hosted-UI problem

In concrete terms, `/mcp` should continue accepting the current token-presented auth modes:

- static bearer token auth
- OAuth-issued `lab` JWT access tokens presented as bearer tokens

The first cut should not add browser session-cookie auth to `/mcp`.

### Auth Resolution Order

For `/v1/*`, the preferred resolution order is:

1. bearer token
2. browser session cookie
3. unauthenticated

Bearer-first resolution preserves explicit machine/client auth and avoids surprising precedence inversions when an operator has both a browser session and a bearer token in play.

## New Backend Endpoints

The hosted UI needs a small browser-oriented auth surface in addition to the existing OAuth endpoints.

### `GET /auth/session`

Purpose:

- tell the browser whether the current request is authenticated
- return minimal operator-facing identity/session state

Expected shape:

- authenticated: `{ "authenticated": true, "user": { ... }, "expires_at": ... }`
- unauthenticated: `{ "authenticated": false }`

This endpoint must be safe for the browser to call on first paint.

### `POST /auth/logout`

Purpose:

- revoke the current browser session
- clear the session cookie

The response should be success-oriented and idempotent.

### `GET /auth/login`

The hosted browser UI should start from a dedicated Rust-owned login entrypoint instead of redirecting directly to `/authorize`.

That keeps the browser-session flow separate from the existing OAuth client contract and gives Rust one stable place to:

- preserve the requested post-login destination
- branch into browser-session logic without changing the OAuth client flow
- keep future provider-specific logic out of the frontend

## Frontend Changes

The frontend work becomes intentionally small.

### Auth State Model

Labby should track only:

- unknown/loading
- authenticated
- unauthenticated

The source of truth is `GET /auth/session`, not browser token storage.

### Login UX

When unauthenticated, the UI should show a real signed-out state and a login action that performs a full-page redirect to the Rust-owned login path.

### Logout UX

Logout should:

- call `POST /auth/logout`
- clear local UI auth state
- return the user to the signed-out view

### API Client Behavior

The browser API client should use same-origin requests with credentials included.

It should not:

- attach a bearer token from browser storage
- own a refresh-token flow
- own OAuth callback parsing or PKCE state

## CSRF Protection

Cookie-based auth introduces CSRF risk for browser-initiated state-changing requests.

This design requires explicit CSRF protection for session-authenticated browser writes.

Acceptable first-cut approaches include:

- synchronizer token pattern with a readable CSRF token endpoint/header
- double-submit cookie pattern

The important contract is:

- safe read requests can rely on the session cookie alone
- state-changing requests must require a CSRF proof not automatically supplied by a third-party site

This protection should apply only to session-cookie auth flows, not to bearer-token API calls.

## Session and Identity Data

The browser session should carry enough identity to support the hosted UI and auditing, but not more.

The session/auth context exposed to API handlers should include at least:

- subject/user identifier
- display identity or email when available
- auth mode: `session` vs `bearer`
- issuer/source where useful for audit logs

The browser session endpoint should expose only what the UI needs to render operator identity and signed-in state.

## Observability

This feature adds a second successful auth path for `/v1/*`, so observability must distinguish them.

Required logging shape additions:

- browser session creation
- browser session revocation
- browser session auth success/failure on `/v1/*`
- explicit auth mode in request context or structured fields where appropriate

Sensitive values must still never be logged:

- cookies
- bearer tokens
- refresh tokens
- authorization codes

## Error Handling

### Browser-visible behavior

The UI should receive clear outcomes for:

- not signed in
- session expired
- session invalid
- CSRF failure

These should be rendered as actionable states, not generic backend failure banners.

### Backend semantics

Suggested semantics:

- missing or invalid browser session -> `401`
- missing/invalid CSRF proof on session-authenticated write -> `403`
- bearer-token failures remain `401` with existing metadata hints where applicable

## Migration Plan Shape

### Phase 1: Session primitives in Rust

- add session model/store to `crates/lab-auth`
- add cookie issuance/validation
- add session-status and logout endpoints

### Phase 2: API middleware integration

- teach `/v1/*` auth middleware to accept browser sessions
- keep bearer-first fallback behavior
- keep `/mcp` bearer-only
- add CSRF protection for session-backed writes

### Phase 3: Frontend integration

- add signed-out screen and login redirect
- add session bootstrap on app load
- switch browser API client to same-origin credentialed requests
- remove hosted-mode dependence on `NEXT_PUBLIC_API_TOKEN`

### Phase 4: Documentation and cleanup

- document hosted UI auth behavior
- document that `NEXT_PUBLIC_API_TOKEN` is dev-only
- update startup/operator docs for the browser session model

## Verification

The design is complete only when all of the following are true:

- hosted Labby loads without embedded bearer tokens
- an unauthenticated user is redirected into the Rust-owned OAuth flow
- successful login returns to the hosted UI with a working browser session
- `/v1/*` works for the browser via session cookie
- `/v1/*` still works for automation via bearer tokens
- `/mcp` still works with token-presented auth, including OAuth-issued `lab` JWT bearer tokens, and is not coupled to browser sessions
- session-authenticated write requests are CSRF-protected
- logout revokes the browser session and returns the UI to a signed-out state

## Risks

### Dual auth-path complexity

Supporting both bearer and session auth on `/v1/*` increases middleware and test complexity. This is acceptable because it cleanly separates browser and programmatic use cases.

### CSRF mistakes

Cookie auth is materially easier to misuse than bearer auth. The mitigation is to make CSRF protection part of the design, not a later hardening pass.

### Scope drift into MCP

There will be pressure to let browser sessions authenticate `/mcp` too. That should be resisted unless there is a concrete product need, because it expands scope and muddies the MCP contract.

### Session-store coupling

If browser sessions are implemented outside `crates/lab-auth`, auth ownership will split across the repo again. The mitigation is to keep browser session primitives in the Rust auth subsystem.

## Backlog Follow-Ons

These are intentionally deferred from the first cut:

- richer identity/profile UI
- multi-tab login sync niceties
- session management UI
- browser access to `/mcp`
- non-same-origin hosted UI support
