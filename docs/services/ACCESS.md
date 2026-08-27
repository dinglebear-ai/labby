---
title: "Access Owner Bootstrap"
created: "2026-08-23"
updated: "2026-08-23"
---

# Access Owner Bootstrap

When OAuth browser mode is configured, Labby mounts one deliberately narrow HTTP workflow for creating the first access-control owner:

```http
POST /v1/access/bootstrap-owner
Content-Type: application/json
X-CSRF-Token: <browser-session token>

{
  "organization_name": "Local",
  "project_name": "Default"
}
```

This is not a registered multi-surface service or a general policy-mutation API. It has no CLI, MCP, Code Mode, stdio, or bearer-automation projection.

## Authorization boundary

The request succeeds only when every gate is satisfied:

- OAuth browser mode is configured;
- the normal `/v1` middleware validates the `lab_session` cookie and matching `X-CSRF-Token`;
- that middleware supplies both `AuthContext` and a canonical `VerifiedIdentity` derived from the authenticated session;
- the session carries `lab:admin`; and
- the authenticated email matches the configured `LABBY_AUTH_ADMIN_EMAIL` case-insensitively.

The configured email is an eligibility gate for this initial operation, not the durable Principal key. The stored owner link comes from the middleware-derived canonical provider issuer and subject. Request JSON cannot provide or override identity.

A static bearer, OAuth bearer without a browser session, local credential, Unix peer, MCP request, CLI/stdio invocation, forged handler extension, or loopback origin cannot bypass these gates. Loopback placement is not authentication.

## Behavior and responses

Both names are trimmed, limited to 128 bytes, and reject empty or control-containing values. A successful transaction creates the reserved local Organization, owner Principal and canonical identity link, default Project owner membership, audit event, and bootstrap metadata atomically.

Success is intentionally redacted:

- `201 {"status":"created"}` means the transaction created the bootstrap state.
- `200 {"status":"already_applied"}` means the same identity and names were already applied.

The response never returns Principal IDs, provider subjects, identity fingerprints, policy rows, or database details. All handler responses use `Cache-Control: private, no-store` and the canonical agent error envelope; authentication and CSRF rejections retain the shared auth-middleware envelope. A missing or invalid browser session returns `401`; a missing or invalid CSRF token, malformed JSON, or invalid name returns `422`; authorization failures return `403`; conflicts return `409`; unavailable, busy, or integrity-failing storage returns `503`. When OAuth browser mode is absent, the route is not mounted and the ordinary router fallback returns `404` before request-body validation.

Setup and doctor inspect access-store health read-only. They do not call this endpoint or silently bootstrap/repair authorization state.

## Related docs

- [Access-control specification](../access-control/SPEC.md)
- [Access-control data model](../access-control/DATA_MODEL.md)
- [OAuth runtime](../runtime/OAUTH.md)
- [Setup service](./SETUP.md)
- [Doctor service](./DOCTOR.md)
