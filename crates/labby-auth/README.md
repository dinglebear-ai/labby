# labby-auth

`labby-auth` supplies authentication, identity, token, session, and OAuth state
for internal Labby consumers (`publish = false`). The consumer owns service
startup, product resource ownership, Project membership, and action authorization.

Read the [crate instructions](AGENTS.md), [architecture](../../docs/ARCH.md),
and [access contract](../../docs/services/ACCESS.md) before changing that boundary.

## Authentication flows

Inbound provider OAuth authenticates a person through one selected identity
provider: Google or Authelia. Labby validates that provider's identity evidence,
applies its local admission policy, and issues its own credentials. Provider
ID tokens and refresh credentials are distinct from Labby access/refresh tokens.
Google credential brokering can reuse one provider credential across downstream
clients; Authelia refresh renewal uses local Labby policy and does not store an
Authelia refresh token.

Labby-issued access tokens use Ed25519 signatures with the JWT `EdDSA`
algorithm. Google ID-token verification uses `RS256`; changing the local signer
does not change Google's verification contract. Browser sessions use opaque
server-side session identifiers and CSRF state, rather than placing provider
or Labby bearer tokens in browser storage.

Outbound upstream OAuth lets Labby act as an OAuth client of a remote MCP server.
Credentials, callback state, refresh coordination, and cached clients bind to
an upstream and subject. An explicit Google-provider source can share the
brokered credential; inbound and outbound stores and encryption keys remain separate.

See [HTTP auth modes](../../docs/runtime/OAUTH.md),
[inbound provider selection](../../docs/design/INBOUND_IDENTITY_PROVIDER.md),
[Google credential broker](../../docs/design/GOOGLE_CREDENTIAL_BROKER.md), and
[upstream runtime](../../docs/services/UPSTREAM.md) for the owning contracts.

## Public interfaces and ownership

[src/lib.rs](src/lib.rs) declares the module and re-export surface:

| Interface | Responsibility |
| --- | --- |
| `config::{AuthConfig, AuthConfigBuilder, AuthModeConfig}` | Parse caller-supplied values, parameterize consumer conventions, and validate configuration. |
| `state::AuthState` | Shared OAuth configuration, store, signing keys, selected provider/generation, resource registry, rate limits, and bounded cleanup. |
| `sqlite::SqliteStore`, `types` | Persisted clients, authorization state, local refresh/session state, identity-provider bindings, allowlists, and upstream credentials. |
| `jwt::{SigningKeys, AccessClaims}`, `at_rest::TokenEncryptionKey` | Local access JWTs/JWKS and provider/replay encryption. |
| `google`, `oauth_provider`, `authelia` (`http-axum`) | Provider exchanges and verified identity evidence; Authelia discovery/HTTP support requires the HTTP feature. |
| `VerifiedIdentity`, `PrincipalLink`, `Authenticator` | Verified transport facts and stable external issuer/subject or local-credential links. Email is not a Principal key. |
| `resource_registry::ResourceRegistry` | Configured OAuth resource scopes plus bounded temporary resource leases. Configuration replacement preserves independent leases. |
| `AuthLayer`, `AuthContext`, `RequiredScopes` (`http-axum`) | Protected-route authentication, caller context, scope checks, and consumer-supplied authorization/revalidation hooks. |
| `routes`, `authorize`, `token`, `metadata`, `cimd` (`http-axum`) | OAuth endpoints, registration/metadata, grant exchange/revocation, and client metadata handling. |
| `session`, `static_session` (`http-axum`), `project_session` | Provider-bound sessions, process-local static-bearer browser sessions, and durable Project-bound browser sessions. |
| `browser_authority`, `reauth`, `reauth_browser` (`http-axum`) | Revalidated browser grants and purpose-bound reauthentication proofs; the consuming policy supplies authorization. |
| `depot_delegation`, `managed_depot_delegation` | Separate exact-operation delegation protocols and Ed25519 signers. Signing does not authorize the request or activate managed mode. |
| `transport_grant`, `trusted_host` | Bound encrypted transport grants and verification of delegated host identity; HTTP middleware is feature-gated. |
| `upstream` (`upstream-oauth-rmcp`) | Outbound OAuth managers, encrypted stores, single-flight refresh, per-subject client cache, and runtime builders. |
| `error`, `util`, `pages` (`http-axum`) | Typed failures, shared security helpers, and escaped human-facing OAuth documents. |

`AuthLayer` hooks include `ProductAccessGrantResolver` and `ProjectSessionRevalidator`.
`BrowserAuthority` requires a `BrowserPolicy`; a session does not grant every permission.

## Features

[Cargo.toml](Cargo.toml) sets `default = []`. The features are independent:

| Feature | Additional compiled surface |
| --- | --- |
| `http-axum` | Axum/Tower middleware, auth context, OAuth routes, browser handlers, Authelia, and pages. |
| `upstream-oauth-rmcp` | Outbound MCP OAuth runtime and its rmcp-client, oauth2, anyhow, and `labby-runtime` dependencies. |
| `testkit` | Explicit daemon-fixture injection hooks; production consumers leave this off. |

Storage, JWTs, configuration, identity, resource registry, and other ungated
interfaces remain available with empty defaults. Enabling outbound OAuth does
not imply Axum handlers. `--all-features` includes fixture hooks as well as both
runtime surfaces; a plain test command omits the HTTP and upstream suites.
The product's `proxy-testkit` feature enables this crate's `testkit` feature.

## Configuration and initialization

`AuthConfigBuilder::build_from_sources` consumes explicit environment-style
key/value pairs. It does not load product TOML or dotenv files. The compatibility
default prefix is `LAB`; the Labby product supplies `LABBY`, its installation
directory, and additional product defaults. Set the prefix, data directory,
scopes, resource/login paths, cookie name, and registration policy explicitly
when adapting the crate to another consumer. Dynamic registration defaults to
false in the reusable builder; the product can opt in.

Build a validated OAuth configuration before calling `AuthState::new`: provide
its public URL, selected provider, admin allowlist, and persistent encryption
key. Google requires client credentials and a registered callback. Authelia
needs `http-axum` and trusted issuer discovery/token/JWKS endpoints.
Its discovery preflight completes before the database opens or migrates.

This `http-axum` helper accepts configuration values and an owned state directory.
It runs inside a Tokio runtime and returns a router without binding a listener:

```rust
use labby_auth::{config::AuthConfigBuilder, error::AuthError, state::AuthState};
use std::path::PathBuf;

async fn oauth_router(
    values: Vec<(String, String)>,
    state_dir: PathBuf,
) -> Result<axum::Router, AuthError> {
    let config = AuthConfigBuilder::new()
        .env_prefix("LABBY")
        .default_data_dir(state_dir)
        .build_from_sources(values)?;
    let state = AuthState::new(config).await?;
    Ok(labby_auth::routes::router(state))
}
```

Initialization opens/migrates SQLite, loads or creates signing material,
activates the selected provider generation, and starts bounded expired-record
cleanup. Dropping the last shared cleanup owner aborts that task. Provider
changes can invalidate persisted authorization/session state. Constructing
`AuthState` is therefore a runtime action, not a read-only configuration check.

`routes::router` mounts OAuth/browser endpoints; it does not protect arbitrary
application routes. Apply `AuthLayer` to those routes with the correct resource,
required scopes, cookie policy, and product hooks. `routes::bearer_only_router`
provides the protocol subset without browser-only routes; its name does not
mean static-bearer-only authentication. `auth_route_specs` owns the
provider-aware route inventory. Use `new_with_resource_registry` when another
runtime must share configured scopes and resource leases with OAuth issuance.

Outbound builders in [upstream/runtime.rs](src/upstream/runtime.rs) accept
`UpstreamConfig` values, `AuthConfig`, and explicit encryption material. They
return no runtime when no upstream declares OAuth. The public-callback builder
requires an explicit HTTPS base; `build_upstream_oauth_runtime_with_redirect`
accepts a host-provided callback URI. The host owns listener/browser handling,
consumer authorization, and lifecycle invalidation around these builders.

## Persistence, crypto, and refresh invariants

`SqliteStore::open` and `open_with_key` can create/migrate a database, clean
expired rows, and encrypt legacy provider rows. SQLite work uses bounded
admission and blocking workers, WAL mode, foreign keys, and a busy timeout.
Versioned migrations in [sqlite/migrations.rs](src/sqlite/migrations.rs) reject
a newer schema. Preserve the database and WAL/SHM sidecars together, and cover
migration changes with the owning store tests.

`SigningKeys::load_or_create` persists a restricted Ed25519 PKCS#8 key and
publishes its public JWKS. Existing non-Ed25519 material is retired and replaced;
previous local tokens do not survive that signer change. Access-token
validation checks the algorithm, signature, audience, issuer, time claims, and
authorized-party format. Consumers still own current authorization decisions.

Inbound at-rest encryption uses a 32-byte key encoded as hex or unpadded
base64url. Outbound `upstream::encryption::load_key` instead accepts standard
base64. Both use ChaCha20-Poly1305 with fresh nonces; context-bound formats bind
credentials to their owner. Preserve these distinct formats and keys. Missing
keys or failed decryption must remain visible errors, not plaintext fallback
for stores that require encryption.

Google credentials belong to the verified account and provider client, not a
downstream DCR/CIMD client. Consent skipping requires exactly one allowed email
with a matching client credential. Terminal `invalid_grant` invalidates the
observed generation and its dependent local grants atomically. Successful
refresh persists the replacement credential before local token rotation;
late failures cannot delete a newer generation. Local refresh replay responses
are encrypted and revalidate current provider binding and admission policy.

Provider activation generations also fence stale callbacks and token issuance.
Outbound cached clients use subject/upstream lifecycle epochs and publication
barriers so revocation cannot republish a stale client. Preserve these guards
when changing concurrency, cleanup, or refresh behavior. The
[Google consent instructions](AGENTS.md#google-consent-forcing-invariant-force_consent)
and refresh regression tests describe the required race behavior.

Secret files use restricted Unix permissions or Windows current-user DACLs
through `labby-winjob`. Storage-protection changes need native platform tests.

## Errors and diagnostics

`AuthError::kind()` supplies stable categories; its `http-axum` mapping adds
no-store headers and rate-limit retry metadata. `upstream::types::OauthError`
retains outbound recovery kinds such as `oauth_needs_reauth` and typed storage
causes. Preserve these when mapping product envelopes; terminal credential
rejection requires reauthorization rather than a retry of the rejected token.

Never log raw credentials, authorization codes, OAuth state, or session secrets.
`util::fingerprint` is for non-secret metadata. Secret correlation uses the
process-local, domain-separated `util::secret_diagnostic_id`; unavailable
randomness produces an unavailable diagnostic ID rather than a stable secret
hash. Follow the [error contract](../../docs/dev/ERRORS.md).
For browser asset maintenance, use [OAuth browser documents](src/pages/README.md);
generated page assets have their own source and validation contract.

## Verification

Run from the workspace root with the pinned toolchain:

```bash
cargo check -p labby-auth --no-default-features --all-targets
cargo check -p labby-auth --no-default-features --features http-axum --all-targets
cargo check -p labby-auth --no-default-features --features upstream-oauth-rmcp --all-targets
cargo nextest run -p labby-auth --all-features
cargo nextest run -p labby-auth --all-features --lib jwt::
cargo nextest run -p labby-auth --all-features --lib upstream::refresh_regression_tests
cargo nextest run -p labby-auth --all-features --test verified_identity --test resource_registry
cargo nextest run -p labby-auth --no-default-features --features http-axum --lib authorize::
cargo nextest run -p labby-auth --no-default-features --features upstream-oauth-rmcp --lib upstream::
```

The combined-feature refresh regressions require both surfaces. Fixture tests
do not qualify a live identity provider; [Testing](../../docs/dev/TESTING.md)
owns broader gates and native/live evidence. Nextest excludes doctests;
`just rustdoc-check` checks workspace Rustdoc and doctests. README Rust fences
are examples, not executable doctests.
