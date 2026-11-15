# Real authorization and session checks

The Playwright `e2e-real/authorization.spec.mjs` runs against the disposable Rust daemon and Phoenix app started by the real harness. It never intercepts requests, injects identities, changes the access database, or calls fixture endpoints.

Coverage authored:

- Anonymous navigation to seven protected LiveView pages redirects to sign-in and renders no protected model; direct `gateway.list` dispatch returns HTTP 401.
- An invalid bearer exchange returns HTTP 401 and establishes no console session.
- Owner browser sign-out prevents access even when the previously captured, signed console cookie is replayed in a new context.
- A real gateway browser cookie introspects as authenticated; logout without its CSRF token returns HTTP 422 and preserves the session; valid logout returns HTTP 204 and replaying the old cookie cannot dispatch gateway reads.

The protocol assertions come from `crates/labby/src/api/browser_session.rs` (`static_browser_exchange_introspection_and_logout`) and the production auth middleware, rather than fixture response expectations. Requests are serial, retries are disabled, HTTP steps have 10-second deadlines, and the suite uses the harness's generated disposable credential. Credentials and cookie values are not written to logs or traces.

## Restricted caller boundary

`restricted-start.mjs` provisions a second real stock Labby daemon on loopback port 4301 using the existing public workflow exercised by `crates/labby/tests/support/live_identity.rs`. It creates a private installation, prepares one bootstrap proof/credential with the shipped CLI, publishes a Gateway Loadout and protected MCP route with `lab:read`, then consumes the proof through `/auth/bootstrap/consume`. Credential introspection must confirm the effective scope ceiling before startup succeeds. No access database is edited and no middleware identity is injected.

The fifth Playwright journey verifies real credential introspection and project/route binding, a Loadout-filtered catalog without administrator actions, HTTP 403 for server logs, project browser session creation and catalog access, and HTTP 401 for both credential and session after real credential revocation. The bootstrap product principal owns its disposable installation but its transport credential is restricted to `lab:read`; this tests a real scope ceiling, not a distinct member-role matrix.

The current console accepts only static setup credentials through `/auth/bearer-session`. It cannot yet exchange a project credential through `/auth/local-session`; the journey explicitly checks that unsupported console sign-in fails closed. Project sessions are exercised through the real supported HTTP endpoint with its required Host/Origin and bound product credential. No external OAuth provider is contacted.

The root harness invokes the helper's `cleanup()` before stopping the daemon. Cleanup uses the exact one-time proof and prepare ID through the supported endpoint, then removes only the owned proof and credential outputs. A failure is reported rather than claiming revocation. The process/config/state ownership remains with the root harness.

## Execution receipt

The file passes `node --check`. The coordinator owns the serial real-backend browser execution and records terminal results; this document does not claim an unrun browser pass.
