# Labby Browser Session Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Rust-owned browser session/cookie auth for the hosted Labby UI so the static frontend can use `/v1/*` without embedded bearer tokens, while `/mcp` stays token-authenticated.

**Architecture:** Extend `crates/lab-auth` to own browser session issuance, validation, and revocation on top of the existing OAuth server. Add a dedicated browser-login entrypoint instead of reusing the existing OAuth client `/authorize` contract. Teach the `lab` HTTP API middleware to accept either a presented bearer token or a browser session cookie on `/v1/*`, while keeping `/mcp` on the current token-presented auth path only. Keep the frontend static and thin by replacing hosted `NEXT_PUBLIC_API_TOKEN` dependence with same-origin credentialed requests and a session-status bootstrap.

**Tech Stack:** Rust (`axum`, `tokio`, `rusqlite`, existing `lab-auth`), Next.js static export, React 19, existing gateway API client utilities.

---

## File Structure

### Rust auth ownership

- Create: `crates/lab-auth/src/session.rs`
  Browser-session primitives: cookie name helpers, session creation/lookup/revoke API, CSRF token helpers, and response-cookie helpers.
- Modify: `crates/lab-auth/src/sqlite.rs`
  Add durable browser-session persistence and cleanup.
- Modify: `crates/lab-auth/src/types.rs`
  Add browser-session row/view types.
- Modify: `crates/lab-auth/src/state.rs`
  Expose browser-session config/state alongside existing OAuth state.
- Modify: `crates/lab-auth/src/authorize.rs`
  Preserve the existing OAuth client flow while adding browser-login/session issuance helpers used by the mounted `lab` HTTP surface.
- Modify: `crates/lab-auth/src/lib.rs`
  Export new session module/types.

### HTTP API wiring

- Modify: `crates/lab/src/api/router.rs`
  Accept browser session cookies on `/v1/*`, keep `/mcp` token-only, add `GET /auth/login`, `GET /auth/session`, and `POST /auth/logout`, and add CSRF enforcement for session-backed writes.
- Modify: `crates/lab/src/api/state.rs`
  Carry any session-specific app state needed by router helpers.
- Modify: `crates/lab/src/api/oauth.rs`
  Add shared request-extension/auth-context helpers if session and bearer auth need a normalized representation.

### Frontend

- Modify: `apps/gateway-admin/lib/api/gateway-request.ts`
  Keep same-origin `credentials: 'include'` as the hosted default and retain `NEXT_PUBLIC_API_TOKEN` only as an explicit dev override.
- Create: `apps/gateway-admin/lib/auth/session.ts`
  Small browser client for `GET /auth/session` and `POST /auth/logout`.
- Create: `apps/gateway-admin/components/auth/auth-bootstrap.tsx`
  Client boundary that fetches session state and gates hosted pages.
- Modify: `apps/gateway-admin/app/(admin)/layout.tsx`
  Render the client bootstrap boundary from the server layout.
- Optionally create: `apps/gateway-admin/components/auth/login-screen.tsx`
  Central signed-out state and login button that redirects into Rust-owned auth.

### Tests

- Modify: `crates/lab-auth/src/sqlite.rs`
  Add store tests for browser sessions.
- Modify: `crates/lab/src/api/router.rs`
  Add HTTP auth tests for session-backed `/v1/*`, token-backed `/mcp`, and CSRF failures.
- Create: `apps/gateway-admin/lib/api/session.test.ts`
  Add frontend session bootstrap/logout tests.
- Modify: `apps/gateway-admin/lib/api/gateway-request.test.ts`
  Update request-init expectations for hosted cookie mode.

---

### Task 1: Add browser-session data model and persistence

**Files:**
- Create: `crates/lab-auth/src/session.rs`
- Modify: `crates/lab-auth/src/types.rs`
- Modify: `crates/lab-auth/src/sqlite.rs`
- Modify: `crates/lab-auth/src/lib.rs`
- Test: `crates/lab-auth/src/sqlite.rs`

- [ ] **Step 1: Write the failing store tests**

Add tests near the existing SQLite-store tests for:

```rust
#[tokio::test]
async fn browser_session_round_trip_succeeds() {
    let store = temp_store().await;
    let row = BrowserSessionRow {
        session_id: "sess_123".into(),
        subject: "user_1".into(),
        email: Some("jmagar@example.com".into()),
        csrf_token: "csrf_123".into(),
        created_at: 1,
        expires_at: 9999999999,
    };

    store.upsert_browser_session(row.clone()).await.unwrap();
    let fetched = store.find_browser_session("sess_123").await.unwrap().unwrap();

    assert_eq!(fetched.session_id, row.session_id);
    assert_eq!(fetched.subject, row.subject);
    assert_eq!(fetched.csrf_token, row.csrf_token);
}

#[tokio::test]
async fn revoking_browser_session_removes_it() {
    let store = temp_store().await;
    let row = BrowserSessionRow {
        session_id: "sess_123".into(),
        subject: "user_1".into(),
        email: None,
        csrf_token: "csrf_123".into(),
        created_at: 1,
        expires_at: 9999999999,
    };

    store.upsert_browser_session(row).await.unwrap();
    store.revoke_browser_session("sess_123").await.unwrap();

    assert!(store.find_browser_session("sess_123").await.unwrap().is_none());
}
```

- [ ] **Step 2: Run the failing tests**

Run: `cargo test --manifest-path crates/lab-auth/Cargo.toml browser_session -- --nocapture`

Expected: FAIL with missing `BrowserSessionRow` type and/or missing SQLite store methods.

- [ ] **Step 3: Add minimal session types and SQLite methods**

Implement:

- `BrowserSessionRow` in `types.rs`
- `upsert_browser_session`
- `find_browser_session`
- `revoke_browser_session`
- table creation / cleanup logic in `sqlite.rs`
- `pub mod session;` or re-exports in `lib.rs`

Minimal row shape:

```rust
pub struct BrowserSessionRow {
    pub session_id: String,
    pub subject: String,
    pub email: Option<String>,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
}
```

- [ ] **Step 4: Re-run the tests**

Run: `cargo test --manifest-path crates/lab-auth/Cargo.toml browser_session -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/types.rs crates/lab-auth/src/sqlite.rs crates/lab-auth/src/session.rs crates/lab-auth/src/lib.rs
git commit -m "feat: add browser session persistence"
```

---

### Task 2: Issue and revoke browser sessions in `lab-auth`

**Files:**
- Modify: `crates/lab-auth/src/session.rs`
- Modify: `crates/lab-auth/src/state.rs`
- Modify: `crates/lab-auth/src/authorize.rs`
- Modify: `crates/lab/src/api/router.rs`
- Test: `crates/lab-auth/src/authorize.rs`

- [ ] **Step 1: Write the failing browser-login/session tests**

Add tests covering:

```rust
#[tokio::test]
async fn browser_login_starts_upstream_flow_without_using_oauth_client_authorize() {
    // call GET /auth/login
    // assert redirect goes to Google authorize URL
    // assert post-login destination is preserved in server-owned state
}

#[tokio::test]
async fn logout_clears_browser_session_cookie() {
    // call logout handler
    // assert cookie is expired/cleared
}
```

- [ ] **Step 2: Run the failing tests**

Run: `cargo test --manifest-path crates/lab-auth/Cargo.toml browser_login -- --nocapture`

Expected: FAIL because the browser-login entrypoint and session-cookie behavior do not exist yet.

- [ ] **Step 3: Implement minimal browser-login and session issuance primitives**

In `session.rs`, add:

- opaque session-id generation
- CSRF-token generation
- cookie helper builders
- `create_browser_session(...)`
- `clear_browser_session_cookie(...)`

In `state.rs`, add any session TTL and cookie-name configuration needed by `AuthState`.

In `authorize.rs`, preserve the existing OAuth client code path, but add the server-side browser-login state needed so the mounted `lab` HTTP router can start browser auth without reusing `/authorize`.

On successful upstream callback in the browser-login flow:

- create session row
- persist it
- attach `Set-Cookie`
- redirect to `https://lab.example.com/` or same-origin app root

In `crates/lab/src/api/router.rs`, wire:

- `GET /auth/login`
- `GET /auth/session`
- `POST /auth/logout`

- [ ] **Step 4: Re-run the tests**

Run: `cargo test --manifest-path crates/lab-auth/Cargo.toml browser_login -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/session.rs crates/lab-auth/src/state.rs crates/lab-auth/src/authorize.rs crates/lab/src/api/router.rs
git commit -m "feat: add browser login and session issuance"
```

---

### Task 3: Accept browser sessions on `/v1/*` and keep `/mcp` token-only

**Files:**
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/oauth.rs`
- Test: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Write failing router tests**

Add tests for:

```rust
#[tokio::test]
async fn v1_accepts_browser_session_cookie() {
    // build router with oauth state + seeded browser session
    // call a protected /v1 route with Cookie header only
    // expect non-401
}

#[tokio::test]
async fn mcp_rejects_browser_session_cookie_without_bearer() {
    // call /mcp with session cookie but no bearer header
    // expect 401 auth_failed
}

#[tokio::test]
async fn v1_post_rejects_missing_csrf_for_session_auth() {
    // POST to /v1/gateway with session cookie and no CSRF proof
    // expect 403
}
```

- [ ] **Step 2: Run the failing router tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml v1_accepts_browser_session_cookie -- --nocapture`

Expected: FAIL because the current middleware only understands presented bearer tokens.

- [ ] **Step 3: Implement minimal dual-mode auth**

In `router.rs`:

- keep current token-presented auth path intact
- on `/v1/*` only, fall back to session-cookie lookup when bearer auth is absent or invalid
- attach a normalized auth context with `auth_mode = "session"` or `auth_mode = "bearer"`
- add CSRF enforcement for state-changing requests authenticated via session cookie
- keep `/mcp` on token-presented auth only

In `api/state.rs` / `api/oauth.rs`:

- add any helpers needed to read browser session state from `AuthState`
- keep handler code auth-transport-agnostic

- [ ] **Step 4: Re-run the router tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml cli:: --all-features`

Then run:

`cargo test --manifest-path crates/lab/Cargo.toml v1_accepts_browser_session_cookie -- --nocapture`

Expected: PASS for the new tests

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/router.rs crates/lab/src/api/state.rs crates/lab/src/api/oauth.rs
git commit -m "feat: add browser session auth to v1 api"
```

---

### Task 4: Add browser session status and logout contract

**Files:**
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab-auth/src/types.rs`
- Test: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Write the failing contract tests**

Add tests asserting:

```rust
#[tokio::test]
async fn auth_session_reports_authenticated_user() {
    // seed a browser session
    // GET /auth/session with Cookie
    // expect {"authenticated":true,...}
}

#[tokio::test]
async fn auth_session_reports_signed_out_without_cookie() {
    // GET /auth/session
    // expect {"authenticated":false}
}
```

- [ ] **Step 2: Run the failing tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml auth_session_reports_ -- --nocapture`

Expected: FAIL because the route or response shape is missing.

- [ ] **Step 3: Implement the minimal endpoint shapes**

Return stable JSON:

```json
{ "authenticated": false }
```

and

```json
{
  "authenticated": true,
  "user": {
    "sub": "user_1",
    "email": "jmagar@example.com"
  },
  "csrf_token": "csrf_123"
}
```

`POST /auth/logout` should:

- revoke current session if present
- clear cookie
- return `{ "ok": true }`

- [ ] **Step 4: Re-run the tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml auth_session_reports_ -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/types.rs crates/lab/src/api/router.rs
git commit -m "feat: add browser session status endpoints"
```

---

### Task 5: Switch the hosted frontend to cookie-backed requests

**Files:**
- Modify: `apps/gateway-admin/lib/api/gateway-request.ts`
- Modify: `apps/gateway-admin/lib/api/gateway-request.test.ts`
- Create: `apps/gateway-admin/lib/auth/session.ts`
- Create: `apps/gateway-admin/lib/api/session.test.ts`

- [ ] **Step 1: Write the failing frontend tests**

In `gateway-request.test.ts`, add:

```ts
it('uses include credentials when no embedded token is provided', () => {
  const init = gatewayRequestInit('gateway.list', {})
  expect(init.credentials).toBe('include')
  expect((init.headers as Record<string, string>).Authorization).toBeUndefined()
})
```

In `session.test.ts`, add:

```ts
it('fetches session status from /auth/session', async () => {
  // mock fetch
  // assert GET /auth/session with credentials include
})
```

- [ ] **Step 2: Run the failing frontend tests**

Run:

```bash
cd apps/gateway-admin
node --test --experimental-strip-types lib/api/gateway-request.test.ts lib/api/session.test.ts
```

Expected: FAIL because session helper does not exist yet.

- [ ] **Step 3: Implement the minimal browser client changes**

In `gateway-request.ts`:

- default to same-origin credentialed requests
- continue supporting `NEXT_PUBLIC_API_TOKEN` only as an explicit dev override

In `session.ts`, add:

```ts
export async function fetchSession() {
  const response = await fetch('/auth/session', {
    method: 'GET',
    credentials: 'include',
    cache: 'no-store',
  })
  return response.json()
}

export async function logout() {
  const response = await fetch('/auth/logout', {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
  })
  return response.json()
}
```

- [ ] **Step 4: Re-run the frontend tests**

Run:

```bash
cd apps/gateway-admin
node --test --experimental-strip-types lib/api/gateway-request.test.ts lib/api/session.test.ts
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/lib/api/gateway-request.ts apps/gateway-admin/lib/api/gateway-request.test.ts apps/gateway-admin/lib/auth/session.ts apps/gateway-admin/lib/api/session.test.ts
git commit -m "feat: switch hosted frontend to browser sessions"
```

---

### Task 6: Add auth bootstrap and signed-out UX to Labby

**Files:**
- Create: `apps/gateway-admin/components/auth/auth-bootstrap.tsx`
- Create: `apps/gateway-admin/components/auth/login-screen.tsx`
- Modify: `apps/gateway-admin/app/(admin)/layout.tsx`
- Optionally modify: `apps/gateway-admin/app/(admin)/page.tsx`
- Test: manual hosted UI smoke test

- [ ] **Step 1: Define the client boundary explicitly**

Because `app/(admin)/layout.tsx` is currently a server component, choose the client bootstrap boundary first:

- create a client component that fetches `/auth/session` on mount
- render that client component from the server layout
- keep the server layout free of direct browser auth logic

- [ ] **Step 2: Implement minimal auth bootstrap**

Create `AuthBootstrap` that:

- fetches `/auth/session` on mount
- shows loading state initially
- shows signed-out screen with a login button when unauthenticated
- renders children when authenticated

Login button behavior:

```ts
window.location.href = '/auth/login'
```

Render `AuthBootstrap` from the admin layout.

- [ ] **Step 3: Smoke-test the signed-out and signed-in states**

Run the hosted UI manually and verify:

- unauthenticated load shows the signed-out state
- login button redirects to Rust-owned browser login
- authenticated load renders the admin shell
- logout returns to the signed-out state

- [ ] **Step 4: Commit**

```bash
git add apps/gateway-admin/components/auth apps/gateway-admin/app/(admin)/layout.tsx
git commit -m "feat: add hosted ui auth bootstrap"
```

---

### Task 7: Update docs and operator guidance

**Files:**
- Modify: `docs/OAUTH.md`
- Modify: `docs/TRANSPORT.md`
- Modify: `apps/gateway-admin/README.md`
- Modify: `README.md`

- [ ] **Step 1: Write the failing docs checklist**

Create a temporary checklist in the commit message draft or notes:

- hosted UI must no longer depend on embedded bearer token
- `/v1/*` accepts browser sessions and bearer tokens
- `/mcp` remains token-presented auth only
- `NEXT_PUBLIC_API_TOKEN` is dev/smoke only
- `LAB_WEB_UI_DISABLE_AUTH` is documented only as a temporary trusted-proxy bypass

- [ ] **Step 2: Verify current docs are stale**

Run:

```bash
rg -n "NEXT_PUBLIC_API_TOKEN|lab serve|/mcp|/v1|browser session|cookie" README.md docs apps/gateway-admin/README.md -S
```

Expected: existing docs describe the old hosted-token path and lack browser-session guidance.

- [ ] **Step 3: Update docs minimally**

Document:

- hosted Labby uses browser session cookies
- browser login is Rust-owned OAuth
- `/v1/*` supports session cookie and presented bearer tokens
- `/mcp` continues to use static bearer or OAuth-issued `lab` JWT bearer tokens
- `NEXT_PUBLIC_API_TOKEN` stays for local development only
- `LAB_WEB_UI_DISABLE_AUTH` is temporary and not the long-term hosted auth model

- [ ] **Step 4: Run a quick doc sanity pass**

Run:

```bash
rg -n "NEXT_PUBLIC_API_TOKEN|browser session|/mcp|/auth/session|/auth/logout" README.md docs apps/gateway-admin/README.md -S
```

Expected: updated wording appears in the right docs.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/OAUTH.md docs/TRANSPORT.md apps/gateway-admin/README.md
git commit -m "docs: describe hosted browser session auth"
```

---

### Task 8: Full verification pass

**Files:**
- Modify: none expected
- Test: workspace verification commands

- [ ] **Step 1: Run Rust auth tests**

Run: `cargo test --manifest-path crates/lab-auth/Cargo.toml --all-features`

Expected: PASS

- [ ] **Step 2: Run API/router tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml --all-features`

Expected: PASS

- [ ] **Step 3: Run frontend tests**

Run:

```bash
cd apps/gateway-admin
pnpm test
```

Expected: PASS

- [ ] **Step 4: Run final compile check**

Run: `cargo check --workspace --all-features`

Expected: PASS

- [ ] **Step 5: Smoke-test hosted runtime manually**

Run:

```bash
cargo run --all-features -- serve
```

Manual checks:

- `/` loads Labby
- signed-out view appears when no session cookie exists
- login round-trip lands back on the app
- authenticated UI can call `/v1/gateway`
- `/mcp` still returns token-auth errors without bearer auth

- [ ] **Step 6: Commit verification-only follow-up if needed**

```bash
git add -A
git commit -m "test: verify browser session auth end to end"
```
