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

## Automatic Viewer membership

An explicit host-owned policy can admit verified browser identities as Viewers of an existing project:

```toml
[auth]
viewer_email_domains = ["example.com"]
viewer_project_id = "existing-project-id"
```

The policy is disabled by default and currently supports Google browser sign-in only. Enabling it with another provider is a configuration error. `LABBY_AUTH_VIEWER_EMAIL_DOMAINS` overrides the domain list; the project remains selected by the host configuration, never by a browser request. This policy is separate from the legacy login/admin allowlist.

After a qualifying verified sign-in, the first authenticated `/v1` request provisions membership using the provider-bound issuer and subject. Email verification must come from the trusted identity provider. Domains match exactly and case-insensitively; subdomains and suffix lookalikes do not qualify. Session email text alone is not evidence of verified domain ownership.

New memberships receive Viewer, not Member or Admin. Existing active roles remain unchanged. Repeat or concurrent admission is idempotent. Disabled or suspended memberships, principals, projects, organizations, and revoked identity links are never reactivated by this policy.

Viewers may discover, read, and publish Team Library artifacts, but Viewer membership does not grant administrative or execution permissions. A domain-only browser session receives read scope; artifact publishing separately checks the project-level publish permission. Static bearer and project-credential requests do not create domain memberships.

The project must already exist and be active. Admission does not bootstrap an organization, create a project, consume owner-link approval, or manufacture an identity for an email address before sign-in.

## Related docs

- [Access-control specification](../access-control/SPEC.md)
- [Access-control data model](../access-control/DATA_MODEL.md)
- [OAuth runtime](../runtime/OAUTH.md)
- [Setup service](./SETUP.md)
- [Doctor service](./DOCTOR.md)
