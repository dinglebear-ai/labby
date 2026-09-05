---
title: "Inbound Identity Provider Contract"
created: "2026-09-05"
updated: "2026-09-05"
---

# Inbound Identity Provider Contract

**Status:** Accepted for implementation
**Scope:** `labby-auth` interactive human login
**Tracking:** `lab-tp2d9`

This document fixes the compatibility and security contract for adding Authelia
as an inbound identity provider. Labby remains the OAuth authorization server
seen by MCP clients. Google and Authelia authenticate the human; neither becomes
the issuer of Labby access tokens.

## Boundaries

- Inbound login lives in `labby-auth` and is distinct from outbound MCP OAuth in
  `labby-auth/src/upstream/` and the public callback relay in
  `crates/labby/src/oauth/`.
- Version 1 permits exactly one active inbound provider per Labby instance:
  Google or Authelia. It has no provider picker or generic OIDC plugin system.
- Provider configuration is read and validated at startup. Hot reload is not
  supported.
- Labby JWT `iss` remains Labby's configured public issuer. The external IdP's
  normalized issuer is carried separately as `identity_issuer`.

## Configuration And Compatibility

The provider is a closed enum, `Google | Authelia`. Existing Google-only
configuration continues to select Google with the same callback, scopes,
hosted-domain behavior, provider credential broker, and consent-forcing rules.
An explicit provider selection takes precedence over legacy inference. A
conflict, partial provider configuration, or explicit selection of an
unconfigured provider fails startup.

Callbacks are fixed contracts:

- Google: `/auth/google/callback`
- Authelia: `/auth/oidc/callback`

The Google path remains a compatibility route, not a provider-neutral alias.
Provider identity and callback identity are bound into one-shot authorization
state, so state created for one provider or callback cannot be replayed at the
other.

`admin_email` is provider-neutral and matches the provider's current verified
email. Google's `hd` claim remains the only basis for Google Workspace domain
authorization. For Authelia, an allowed-domain rule matches the domain of the
verified email claim; it is not equivalent to, and must never be represented
as, a Google `hd` assertion.

## Authelia OIDC Profile

Authelia uses authorization code flow with PKCE S256, nonce, discovery, and
`client_secret_basic`. Requested scopes are `openid profile email`; version 1
does not request `offline_access` or persist an Authelia refresh token.

Discovery is bound to the configured, normalized issuer. The discovered issuer
must match exactly. Authorization, token, and JWKS endpoints must be HTTPS and
must satisfy the configured issuer-origin policy. ID tokens require exact
issuer and audience validation, `azp` validation when applicable, nonce,
expiry/not-before validation, a non-empty subject, and a verified email. The
accepted signing algorithms are an explicit configuration-independent allowlist;
`none` and symmetric algorithms are never accepted.

Public issuers use the hardened public-network HTTP policy. A private Authelia
issuer requires an explicit typed trust capability created from static operator
configuration for one exact normalized HTTPS origin (scheme, host, and effective
port). It does not disable TLS verification, authorize redirects to another
origin, or enable private networking globally.

Discovery and JWKS reads have bounded response sizes, deadlines, redirects,
cache entries, cache lifetimes, refresh attempts, and single-flight behavior.
A warm callback performs no discovery or JWKS network I/O. Unknown-key refresh
is bounded and rate-limited; stale keys are not accepted beyond their declared
policy.

## Durable Identity And Provider Generation

The durable human identity key is `(identity_issuer, subject)`. Email is an
authorization input and mutable profile data, never an identity key.

The database stores one active-provider record containing:

- provider kind;
- normalized external identity issuer;
- client ID;
- fixed callback identity;
- a non-secret configuration fingerprint; and
- a monotonically increasing generation.

The material identity is provider kind + normalized issuer + client ID + fixed
callback. An unchanged restart retains the generation. Client-secret rotation
alone retains it. A material change, including switching providers or switching
away and back, increments the generation transactionally and invalidates every
pending state, code, refresh grant, native result, and browser session from an
older generation. Multiple processes sharing a database must agree with the
durable fingerprint and generation before serving; disagreement fails startup.

Legacy identity-bearing rows are transactionally backfilled with Google's
canonical issuer and the migration generation. Google provider credentials stay
in their existing subject-scoped broker and are not generalized into Authelia
credentials.

## Renewal, Sessions, And Offboarding

Authelia renewal is local-policy-only. Labby stores no Authelia provider refresh
credential and makes no Authelia call while rotating a Labby refresh token or
authorizing an existing browser session. Each renewal performs an indexed point
lookup against current local admin-email, allowed-email, and allowed-domain
policy and requires the current provider generation.

Authelia-side disablement or email changes are observed at the next interactive
login or when the bounded Labby session/refresh lifetime expires. The default
Labby lifetimes remain one hour for access tokens and 30 days for refresh tokens;
configured values are the authoritative maximums.

Provider switching or local-policy removal immediately invalidates pending
authorization state, authorization codes, refresh grants, native results, and
browser sessions. Already-issued stateless Labby access JWTs remain valid for at
most `access_token_ttl`. Signing-key rotation is the emergency global revocation
mechanism and invalidates all outstanding access JWTs, not only one provider.

Google retains its existing live provider-credential refresh and `invalid_grant`
cascade semantics. Authelia does not join that broker.

## Migration And Rollback

The schema change is additive and runs as one serialized transaction. It creates
the provider metadata, adds issuer/generation columns and supporting indexes,
performs set-based Google backfill, validates integrity, and advances
`user_version` last. A failed migration rolls back the v13 transaction;
restart retries safely. A binary that does not understand the advanced schema
must refuse to open it. Downgrade after migration requires restoring a database
backup unless a future release explicitly proves reverse compatibility.

## Failure Behavior

Startup fails closed for malformed or ambiguous provider configuration, an
untrusted/private issuer without the exact trust capability, discovery metadata
that violates issuer/origin policy, or shared-database provider disagreement.
Login fails without minting local state when discovery, JWKS, token exchange, or
claim verification fails. One-shot state and codes are consumed atomically.

Logs and errors never contain client secrets, authorization codes, state, nonce,
PKCE verifiers, tokens, claims, email addresses, subjects, or URLs with query
strings. Metrics use bounded provider/result labels only.

## Non-goals

- simultaneous providers, a provider picker, or dynamic provider registration;
- Authelia group-to-role mapping;
- generic upstream MCP OAuth or callback-relay changes;
- Authelia UserInfo or live IdP revalidation during renewal;
- configurable callback paths;
- RP-initiated logout; or
- unrestricted generic OIDC compatibility.

## Implementation Checklist

- [ ] Closed provider config preserves legacy Google behavior and redacts secrets.
- [ ] Persistence keys identities by issuer and subject and fences generations.
- [ ] Google is adapted without changing its credential-broker invariants.
- [ ] Authelia discovery, HTTP, JWKS, and claim validation obey the bounds above.
- [ ] Authorize, browser, native, refresh, and session paths carry issuer/generation.
- [ ] Product config, setup, doctor, routes, documentation, and catalogs agree.
- [ ] Mock conformance and pinned real-Authelia interoperability both pass.
