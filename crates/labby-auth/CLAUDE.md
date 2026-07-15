# labby-auth — OAuth 2.1 Authorization Server + JWT/Session Crate

`labby-auth` is `publish = false` — it is not distributed via crates.io.
`axon` and `cortex` (sibling homelab repos) each carry their own **vendored
source copy** of this crate rather than a path/git dependency, so a fix
landing here does **not** propagate to them automatically. If you change
behavior that those copies would also need — especially anything touching
the OAuth flows below — leave a note in the PR/commit for whoever next
touches `axon`'s `vendor/lab-auth` or `cortex`'s pinned `lab-auth` git rev
(`Cargo.toml`), and consider whether the fix is worth porting there too.

## Google consent-forcing invariant (`force_consent`)

`authorize()` (`src/authorize.rs`) decides whether to send Google's
`/authorize` request with `prompt=consent` via `AuthState::store::
has_refresh_token_for_client()` and `AuthState::resolve_allowed_emails()`.
This logic has already regressed once (see `git log -S"force_consent"` for
the full history) — the rules that keep it correct:

1. **Google's refresh-token re-issuance is keyed on `(Google account,
   LABBY_GOOGLE_CLIENT_ID)`, not on Labby's local per-client `client_id`.**
   Google only mints a fresh refresh token on a forced-consent round trip
   for an account that hasn't already granted this Google OAuth client
   before. Everything else here is downstream of that one fact.
2. **The "skip consent" check must never be broader than the Google-account
   axis it's approximating.** It was originally a single boolean —
   "has this gateway ever minted a refresh token" — which let an
   established client's token mask a brand-new client's need to force
   consent (a new Claude.ai/ChatGPT/Codex connector would silently get an
   access token with no refresh token and fail with zero server-side
   trace). Scoping it to `has_refresh_token_for_client(client_id)` fixed
   that axis, but is **still** unsound if more than one Google account is
   allowed to sign in (`resolve_allowed_emails().len() > 1`) and two
   different accounts share one local `client_id` — per-client-id state
   can't tell which account is about to authenticate. `authorize()`
   forces consent unconditionally whenever more than one account is
   allowed, specifically to close that gap.
3. **If you add another "skip consent when X" fast path, it must be scoped
   at least as narrowly as `client_id`, and must account for the
   multi-account case in rule 2.** A gateway-wide or subject-agnostic
   check is the exact anti-pattern that caused the original bug.
4. **`force_consent` is logged** (`oauth authorize request redirected to
   upstream provider`, `authorize.rs`) specifically so "a new OAuth client
   silently can't complete setup" is diagnosable from `force_consent=false`
   in the logs instead of requiring a live repro. Keep it logged if this
   logic moves.

## Structure

- `authorize.rs` — `/authorize`, `/register`, `/auth/google/callback`,
  native callback/poll handlers. Route-handler layer.
- `google.rs` — outbound Google OAuth client (`GoogleProvider`): authorize
  URL construction, code exchange, id_token verification, JWKS caching.
- `sqlite.rs` — `SqliteStore`: all persisted OAuth/session state
  (registered clients, authorization codes/requests, refresh tokens,
  browser sessions, allowed-users allowlist, upstream OAuth credentials).
  Versioned migrations live in `run_migrations`, keyed by `PRAGMA
  user_version` / `SCHEMA_VERSION`.
- `token.rs` — `/token` endpoint (authorization_code and refresh_token
  grants).
- `jwt.rs` — `lab` access-token signing/validation (RS256).
- `state.rs` — `AuthState`: shared handle over config, store, signing keys,
  Google provider, and the in-memory allowed-resource-scope map.
- `upstream/` — outbound OAuth for Labby's own upstream MCP connections
  (gated behind the `upstream-oauth-rmcp` feature) — a different concern
  from the inbound `/authorize` flow above; don't conflate the two when
  reasoning about "refresh token" bugs (they're unrelated token stores).

## Feature gates

`default = []`. `authorize.rs`, `token.rs`, `metadata.rs`, `middleware.rs`,
`routes.rs`, and the axum route handlers are gated behind `http-axum`.
`upstream-oauth-rmcp` gates the outbound upstream-OAuth runtime. **Always
verify with `cargo test -p labby-auth --all-features` (or `--features
http-axum`) before trusting a "tests pass" claim for this crate** — a plain
`cargo test -p labby-auth` silently skips every test in `authorize.rs`,
`token.rs`, and friends, with no warning that anything was excluded.
