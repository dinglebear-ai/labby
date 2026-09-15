---
title: "Access Service"
created: "2026-08-23"
updated: "2026-09-13"
---

# Access Service

Labby's durable multi-user authority lives in the access store
(`$LABBY_HOME/access.db`, schema v7; see the
[data model](../access-control/DATA_MODEL.md#schema-v7-current)). This page is
the operator guide for it:

- the registered `access` service (Teams, invitations, platform
  administration, Team credentials);
- onboarding a teammate, including what "No access yet" means;
- owner bootstrap (browser and offline proof), owner identity link, and owner
  recovery;
- the registered `projects` service and the automatic Viewer policy.

Admission and authority are separate. `LABBY_AUTH_ADMIN_EMAIL`, the allowlist,
and the domain settings decide who may sign in. Durable Team, Project, and
platform-administration records decide what a signed-in identity may do. Only
the browser session of `LABBY_AUTH_ADMIN_EMAIL` receives `lab:admin` from
configuration; everyone else gets administrative reach only through
`access.platform_admin.grant`. See
[Browser session scopes](../runtime/OAUTH.md#browser-session-scopes-and-domain-admission).

## Access actions

The generated [action catalog](../generated/action-catalog.md) is authoritative
for the 19 `access` actions (17 domain actions plus `help` and `schema`), their
parameters, required capabilities, and `requires_admin` flags. Do not copy that
table here. In summary:

| Group | Actions | Required capability |
| --- | --- | --- |
| Platform administration | `access.platform_admin.grant`, `access.platform_admin.revoke`, `access.team.create` | `platform.manage`; `requires_admin` (`lab:admin`) |
| Team membership | `access.team.member.add`, `.remove`, `.role.set`, `.suspend`; `access.team.suspend`, `access.team.activate`; `access.team_invitation.create` | `membership.manage` |
| Invitation accept | `access.team_invitation.accept` | `scope.operate` |
| Team resources | `access.team_project.assign`, `access.gateway_credential.bind`, `access.gateway_credential.revoke` | `scope.manage` |
| Reads | `access.team.list`, `access.project.effective.list`, `access.gateway_credential.list` | caller projection / `scope.read` |

Surfaces: the `access` MCP tool and `POST /v1/access/admin`, both with the
shared `action` plus `params` envelope. There is no CLI projection. Browser
sessions must send `X-CSRF-Token` for every action except `help`, `schema`,
`access.team.list`, and `access.project.effective.list`. Every action runs as
the verified caller; `principal_id` and `team_id` parameters select a target
and never establish authority.

```http
POST /v1/access/admin
Content-Type: application/json
X-CSRF-Token: <browser-session token>

{"action": "access.team.member.add",
 "params": {"team_id": "<team>", "principal_id": "<principal>", "role": "member"}}
```

Over MCP, call the `access` tool with the same body:
`{"action": "access.team.list", "params": {}}`.

Team roles are `owner`, `admin`, and `member`. A Team role does not by itself
grant Project authority; `access.team_project.assign` gives the Team an explicit
Project role.

### Error kinds

Access actions use the canonical agent error envelope
(`crates/labby/src/dispatch/access_errors.rs`):

- `forbidden`: the caller lacks authority, or the target does not exist. Absent
  and unauthorized targets return the same denial so identifiers cannot be
  enumerated.
- `invalid_param`: malformed Team, Project Loadout, or bootstrap input.
- `not_found`: no Team Gateway credential binding for that upstream (only after
  Team authority is proven).
- `conflict`: removing or demoting the last active Team owner, a different
  Project Loadout assignment, or conflicting owner bootstrap state.
- `service_unavailable`: the access store is missing, uninitialized, locked,
  read-only, corrupt, insecure, newer than this binary, or busy.

## Onboard a teammate ("No access yet")

A teammate who signs in without durable authority sees "No access yet".
`GET /auth/session` then reports `authority_state: "unprovisioned"` and the
remediation "Ask an administrator to add this identity to a team." Signing out
and in again does not change this. The steps below are what the administrator
does.

1. **Admit the identity.** As the configured admin, add the email to the
   allowlist: the settings UI, or `POST /v1/auth/allowed-emails` with
   `{"email": "teammate@example.com"}`. The allowlist routes accept only the
   browser session of `LABBY_AUTH_ADMIN_EMAIL`. Do not rely on
   `LABBY_AUTH_ALLOWED_EMAIL_DOMAINS` for Google browser access; see the
   [open issue](../runtime/OAUTH.md#domain-allowlist-behavior-by-provider-and-surface).
2. **The teammate signs in.** Browser sign-in alone admits the identity but
   does not create a Principal, so the session is `unprovisioned`.
3. **Get the teammate a Principal.** Today a Principal is created only by:
   - owner bootstrap (the first owner only);
   - MCP team auto-provision: the teammate's first OAuth-delegated request to a
     project-bound protected MCP route whose upstreams include `team-depot`.
     It creates the Principal and a Project membership with role `member`;
   - the [automatic Viewer policy](#automatic-viewer-membership) (Google only):
     the first `/v1` request after a qualifying sign-in creates the Principal
     with a Viewer membership on the host-selected Project. The web UI's
     "No access yet" screen makes that request itself (a read-only
     `GET /v1/catalog`) and reloads the session, so a qualifying teammate
     only needs to sign in.

   There is no action that creates a Principal for a browser-only teammate.
   This is a known product gap.
4. **Learn the `principal_id`.** No action lists Principals. Once the teammate
   has a Principal, their own `GET /auth/session` shows `authority_state:
   "ready"` and `authority.principal_id`. They send you that value.
5. **Add them to a Team** (needs `membership.manage` on that Team, for example
   as its `owner` or `admin`, or platform administration). Either:
   - add directly: `access.team.member.add` with `team_id`, `principal_id`, and
     `role`; or
   - invite: `access.team_invitation.create` with `team_id`, `principal_id`,
     `role` (not `owner`), `token` (an opaque secret you generate), and
     `ttl_seconds`. Give the token to the teammate over a private channel. The
     teammate calls `access.team_invitation.accept` with `{"token": ...}` from
     their own session. Accept succeeds only for the invited Principal, before
     expiry, and only if the Team's membership epoch has not changed since the
     invitation was created (otherwise create a new invitation).
6. **Give the Team a Project**, if it does not have one:
   `access.team_project.assign` with `team_id`, `project_id`, and `role`.
7. **Optional: platform administration.** `access.platform_admin.grant` with
   `principal_id` (requires `lab:admin` and `platform.manage`). The teammate's
   browser session is then elevated to `lab:admin` on `/v1` routes.

### Using a second account (for example work and personal)

Labby keys each Principal on the identity provider's issuer plus subject. A
different Google account is a different Principal, even for the same person.
It signs in as `unprovisioned` and must be onboarded with the steps above like
any teammate, with its own Team memberships. Principals cannot be merged. The
only identity-linking operation is the owner's
[owner identity link](#owner-identity-link).

## Owner bootstrap

The first owner is created exactly once, by one of two entry points that write
the same reserved Organization, owner Principal, identity link, default
Project, owner membership, and audit rows. Both refuse a store that is not
pristine.

### Browser bootstrap

When OAuth browser mode is configured, Labby mounts one narrow HTTP route:

```http
POST /v1/access/bootstrap-owner
Content-Type: application/json
X-CSRF-Token: <browser-session token>

{
  "organization_name": "Local",
  "project_name": "Default"
}
```

This is not a general policy-mutation API. It has no CLI, MCP, Code Mode,
stdio, or bearer-automation projection. The web UI offers owner setup only
when `GET /auth/session` returns `owner_bootstrap_available: true`.

#### Authorization boundary

The request succeeds only when every gate is satisfied:

- OAuth browser mode is configured;
- the normal `/v1` middleware validates the session cookie and matching `X-CSRF-Token`;
- that middleware supplies both `AuthContext` and a canonical `VerifiedIdentity` derived from the authenticated session;
- the session carries `lab:admin`; and
- the authenticated email matches the configured `LABBY_AUTH_ADMIN_EMAIL` case-insensitively.

The configured email is an eligibility gate for this initial operation, not the durable Principal key. The stored owner link comes from the middleware-derived canonical provider issuer and subject. Request JSON cannot provide or override identity.

A static bearer, OAuth bearer without a browser session, local credential, Unix peer, MCP request, CLI/stdio invocation, forged handler extension, or loopback origin cannot bypass these gates. Loopback placement is not authentication.

#### Behavior and responses

Both names are trimmed, limited to 128 bytes, and reject empty or control-containing values. A successful transaction creates the reserved rows and bootstrap metadata atomically.

Success is intentionally redacted:

- `201 {"status":"created"}` means the transaction created the bootstrap state.
- `200 {"status":"already_applied"}` means the same identity and names were already applied.

The response never returns Principal IDs, provider subjects, identity fingerprints, policy rows, or database details. All handler responses use `Cache-Control: private, no-store` and the canonical agent error envelope; authentication and CSRF rejections retain the shared auth-middleware envelope. A missing or invalid browser session returns `401`; a missing or invalid CSRF token, malformed JSON, or invalid name returns `422`; authorization failures return `403` (kind `forbidden`; the message currently distinguishes "caller is not the configured admin" from other denials); conflicts return `409`; unavailable, busy, or integrity-failing storage returns `503`. When OAuth browser mode is absent, the route is not mounted and the ordinary router fallback returns `404` before request-body validation.

Setup and doctor inspect access-store health read-only. They do not call this endpoint or silently bootstrap/repair authorization state.

### Offline proof bootstrap

`labby setup access-bootstrap` (`prepare`, `consume`, `status`, `recover`,
`cleanup`) creates the owner from a one-time 256-bit proof prepared offline
while the installation is pristine, through `POST /auth/bootstrap/consume` and
its sibling `/auth/bootstrap/*` routes. It also issues a project-bound product
credential. Loopback location by itself grants nothing. See
[Local access bootstrap](../guides/LOCAL_ACCESS_BOOTSTRAP.md).

### Existing stores

Owner bootstrap never migrates an existing older-schema store. Upgrading a
v1–v6 store to v7 is the offline `labby state migrate-access` flow in
[MIGRATION.md](../access-control/MIGRATION.md).

## Owner identity link

Owner-link adds one more external identity to the existing owner Principal. It
is the only identity-linking operation; other Principals cannot be linked or
merged.

1. With the gateway stopped, approve the specific identity offline:
   `labby setup owner-link-prepare --approval-file <file>`. The protected JSON
   manifest binds the new identity's fingerprint, the installation, the owner
   Principal, and an existing Project, Loadout, and protected route.
2. Start the gateway. From the new identity's browser session, call
   `POST /v1/access/owner-link/consume` with the session's CSRF token.

Consume accepts only a Google browser session that holds `lab:admin` and whose
verified email equals `LABBY_AUTH_ADMIN_EMAIL`
(`crates/labby/src/api/services/owner_link.rs`). Approval and consumption are
recorded as audit events, and consumption and linking commit in one
transaction.

## Owner recovery

Situations and what to do:

- **The owner's IdP account changes or is lost.** The owner Principal is bound
  to issuer plus subject, not email. Link the replacement account with
  [owner identity link](#owner-identity-link). Because consume requires the
  configured admin email, set `LABBY_AUTH_ADMIN_EMAIL` to the replacement
  account's verified email first (and restart) if it differs.
- **`LABBY_AUTH_ADMIN_EMAIL` was changed after bootstrap.** Changing it moves
  the configured-admin scopes to the new email's browser session, but it does
  not move durable ownership, and bootstrap is refused because an owner exists.
  The new identity is `unprovisioned` until it is linked (above) or onboarded.
- **The access store is damaged.** Restore `access.db` from a WAL-consistent
  backup ([MIGRATION.md](../access-control/MIGRATION.md#backup-and-restore),
  [disaster recovery](../runtime/DISASTER_RECOVERY.md)).
- **The owner's Team membership was suspended.** The catalog has no
  reactivate-member action. Restore from backup, or have another platform
  administrator re-add the owner. Whether `access.team.member.role.set`
  reactivates a suspended membership is not documented; verify before relying
  on it.

Do not delete `access.db` to "reset" access. The runtime then reports setup
required and projects only transport authority, and every Team, membership,
invitation, platform-admin grant, and product credential is lost.

For a suspected privilege exposure, see the
[privilege-exposure runbook](../runtime/PRIVILEGE_EXPOSURE_RUNBOOK.md).

## Projects

The registered `projects` service owns Team-scoped Project lifecycle. It is a
distinct multi-surface service, unlike the bootstrap route above. Its actions
are `projects.list`, `projects.create`, `projects.get`, `projects.update`,
`projects.archive`, and `projects.activate`, exposed over authenticated HTTP at
`POST /v1/projects` with
the shared `action` plus `params` envelope and as the `projects` MCP tool.

Every action runs as the verified Principal; `team_id` and `project_id` params
select the context and never establish authority:

- `projects.list` returns the Projects assigned to Teams where the caller holds
  an active membership. A platform administrator sees every active assignment.
- `projects.get` requires active membership in the named Team.
- `projects.create`, `projects.update`, `projects.archive`, and
  `projects.activate` require a Team manager: an active `owner` or `admin` Team membership, or platform
  administration. Updating, archiving, or activating also requires effective
  Project management authority. Creating a Project assigns it to the Team with the `admin`
  assignment role.
- Absent and unauthorized Team or Project identifiers return the same
  non-enumerating `forbidden` denial.

`projects.archive` sets the Project to `disabled` and advances its policy
epoch; an archived Project leaves `projects.list`. `projects.activate` reverses
it (`disabled` back to `active`, again advancing the policy epoch) in one
transaction and writes an `access.project.activate` audit row. Because archive
is reversible, neither action is classified `destructive`. Activating a Project
that is not archived returns the same non-enumerating denial as an absent one.

## Automatic Viewer membership

An explicit host-owned policy can admit verified browser identities as Viewers of an existing project:

```toml
[auth]
viewer_email_domains = ["example.com"]
viewer_project_id = "existing-project-id"
```

The policy is disabled by default and currently supports Google browser sign-in only. Enabling it with another provider is a configuration error. `LABBY_AUTH_VIEWER_EMAIL_DOMAINS` overrides the domain list; the project remains selected by the host configuration, never by a browser request. This policy is separate from the legacy login/admin allowlist.

After a qualifying verified sign-in, the first authenticated `/v1` request provisions membership using the provider-bound issuer and subject. An unprovisioned session otherwise calls only `/auth/session`, so the web UI's no-access screen issues one read-only `GET /v1/catalog` and then reloads the session; the server alone decides admission. Email verification must come from the trusted identity provider. Domains match exactly and case-insensitively; subdomains and suffix lookalikes do not qualify. Session email text alone is not evidence of verified domain ownership.

New memberships receive Viewer, not Member or Admin. Existing active roles remain unchanged. Repeat or concurrent admission is idempotent. Disabled or suspended memberships, principals, projects, organizations, and revoked identity links are never reactivated by this policy.

Viewers may discover, read, and publish Team Library artifacts, but Viewer membership does not grant administrative or execution permissions. A domain-only browser session receives read scope; artifact publishing separately checks the project-level publish permission. Static bearer and project-credential requests do not create domain memberships.

The project must already exist and be active. Admission does not bootstrap an organization, create a project, consume owner-link approval, or manufacture an identity for an email address before sign-in.

## Related docs

- [Access-control specification](../access-control/SPEC.md)
- [Multi-user authority contract](../access-control/MULTI_USER_AUTHORITY.md)
- [Access-control data model](../access-control/DATA_MODEL.md)
- [Access-control architecture](../access-control/ARCHITECTURE.md)
- [OAuth runtime](../runtime/OAUTH.md)
- [Local access bootstrap](../guides/LOCAL_ACCESS_BOOTSTRAP.md)
- [Privilege-exposure runbook](../runtime/PRIVILEGE_EXPOSURE_RUNBOOK.md)
- [Setup service](./SETUP.md)
- [Doctor service](./DOCTOR.md)
