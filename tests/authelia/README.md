# Authelia acceptance matrix

`just test-authelia` starts the pinned `authelia/authelia:4.39.10` image with
run-unique TLS, signing keys, database state, container name, and host port.
The ignored integration test proves real discovery, first-factor login,
authorization-code PKCE, `client_secret_basic`, nonce/claim validation, JWKS,
and token exchange through Labby's actual MCP callback/token, browser-session,
and native callback/poll routes. It records bounded cold discovery,
token-exchange plus cold-JWKS verification, and same-generation warm
verification timings.

The same gate then runs the complete fast `labby-auth` suite serially. That
suite repeats those entry points with deterministic HTTP fixtures. It also
covers account/allowlist denial, replay and mix-up,
invalid claims, provider-generation replacement, key rotation, discovery/JWKS
failure and last-known-good bounds, redaction, and local-only refresh/offboarding.
The 100-way cold verification test asserts exactly one JWKS fetch; discovery is
performed once when constructing the shared provider generation.

This split keeps ordinary tests container-free while ensuring the dedicated CI
gate cannot pass on the real wire contract without also passing all Labby flow
and fail-closed semantics. Cleanup only removes resources named by the current
run and retains a redacted Authelia log tail when readiness fails.
