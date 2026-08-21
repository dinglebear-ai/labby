# labby-auth — OAuth 2.1 Authorization Server + JWT/Session Crate

`labby-auth` is `publish = false`; it is an internal Labby workspace crate rather than a crates.io package. Keep its public contracts reusable, but do not encode assumptions about sibling repositories or their current dependency strategy in this source-of-truth file.

## Google consent-forcing invariant (`force_consent`)

`authorize()` (`src/authorize.rs`) decides whether to send Google's
`/authorize` request with `prompt=consent` via the subject-scoped
`google_provider_credentials` store and `AuthState::resolve_allowed_emails()`.
This logic has already regressed once (see `git log -S"force_consent"` for
the full history) — the rules that keep it correct:

1. **Google's refresh-token re-issuance is keyed on `(Google account,
   LABBY_GOOGLE_CLIENT_ID)`, not on Labby's downstream `client_id`.** Store
   exactly one encrypted provider credential per verified Google subject and
   let every DCR/CIMD client mint its own Labby refresh token against it.
2. **A single-account gateway may skip consent only when the sole allowed
   email already owns a provider credential.** This is intentionally reusable
   across downstream clients and prevents DCR churn from minting enough Google
   tokens to evict older credentials. With multiple allowed accounts, force
   consent because the selected subject is unknown until callback.
3. **`invalid_grant` is terminal for the exact provider generation that
   failed.** Compare-and-delete that generation, revoke every local refresh
   token and pending authorization code for its subject in the same
   transaction, and force the next authorization through consent. Never keep
   retrying or reattach the rejected credential.
4. **Successful refreshes replace the subject credential before the local
   Labby token rotates.** Generation checks ensure a late failing request cannot
   delete a newer credential installed concurrently.
5. **`force_consent`, provider-credential presence, invalidation generation,
   and revoked dependent counts are logged without raw subjects, emails, or
   tokens.** Keep those fields if this logic moves.

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
