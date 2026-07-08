# Changelog

This project follows a lightweight Keep a Changelog style. Entries here should
focus on operator-visible behavior, released surfaces, and upgrade-relevant
changes rather than internal refactors.

## [Unreleased]

### Added
- Added `LABBY_AUTH_ADMIN_EMAIL` env var (required in oauth mode). The Google
  email of the bootstrap admin permitted to log in. The id_token's
  `email_verified` claim is enforced — unverified accounts are rejected even
  when the address matches. A SQLite-backed multi-user allowlist managed
  through the Labby web UI is planned as a follow-up.
- Added `lab-auth`, a dedicated auth crate for hosted HTTP OAuth flows.
- Added Google-backed OAuth support for the HTTP MCP server, including dynamic
  client registration, PKCE, local JWT issuance, and JWKS publishing.
- Added combined auth-mode support so hosted HTTP can run in either legacy
  bearer mode or OAuth mode.
- Added hosted gateway tool-exposure filtering via `expose_tools`, including
  exact-name and `*` wildcard matching.
- Added the in-repo `apps/gateway-admin` admin UI scaffold for gateway CRUD,
  discovery, and exposure-policy management.
- Added structured OAuth debug logging across registration, authorization,
  callback, and token exchange flows.
- Added config support for allowed non-loopback OAuth client redirect URI
  patterns for callback-relay style clients such as Codex.

### Changed
- **Breaking (auth):** OAuth mode now requires `LABBY_AUTH_ADMIN_EMAIL` to be set.
  Startup fails closed if it is missing. The previous behavior of allowing every
  Google account when no allowlist was configured was a fail-open
  misconfiguration risk. Operators upgrading to this release must add
  `LABBY_AUTH_ADMIN_EMAIL` to their environment before `labby serve`
  will start in oauth mode.
- **Breaking (HTTP API):** `gateway.add`, `gateway.update`, `gateway.remove`, and
  `gateway.reload` are now marked destructive. HTTP and MCP callers must include
  `"confirm": true` in `params` or the request is rejected with
  `kind: "confirmation_required"` (HTTP 422). The in-repo `gateway-admin` UI is
  already updated; any third-party HTTP consumer must add the confirm flag. See
  `docs/GATEWAY.md` for the updated contract and `docs/ERRORS.md` for the
  envelope shape.
- Changed the hosted OAuth flow to advertise and serve `lab` as the auth server
  directly instead of relying on the old external issuer/JWKS configuration.
- Changed Google authorization requests to request offline access with consent
  prompts so fresh logins can yield upstream refresh tokens.
- Changed auth/token documentation and operator docs to reflect the new HTTP
  auth model, gateway exposure controls, and refresh-token behavior.
- Changed config handling so non-secret auth and MCP runtime settings can live
  in `config.toml` instead of only in `.env`.
- Changed the default generated gateway bearer-token env var name to
  `LABBY_GW_{NAME}_AUTH_HEADER`. Existing gateway configs keep using their stored
  env var name; the new default applies when creating new gateway entries.

### Fixed
- Fixed OAuth code, request-state, and refresh-token expiry enforcement.
- Fixed scope handling so OAuth mode accepts only supported scopes instead of
  minting arbitrary client-provided values.
- Fixed `/authorize` validation to require `response_type=code`.
- Fixed refresh-token issuance so `lab` only returns a refresh token when an
  upstream provider refresh token actually exists.
- Fixed symlink traversal handling in static web asset serving.
- Fixed gateway-admin abort handling, loading states, preview behavior, and
  exposure-policy editor reset behavior across gateway switches.
- Fixed scaffold and audit references after the MCP registry moved to
  `crates/lab/src/registry.rs`.

## [0.3.3]

- Current released baseline before the in-progress `Unreleased` changes above.
