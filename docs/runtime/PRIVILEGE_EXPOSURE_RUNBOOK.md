---
title: "Privilege Exposure Runbook"
created: "2026-09-13"
updated: "2026-09-13"
---

# Privilege Exposure Runbook

Use this runbook when administrative scope may have been held by callers who
should not have had it. It also covers restoring a known-good configuration.

## Known exposure: allowlisted browser sessions held `lab:admin`

Before PR #637, every email-allowlisted browser session received the full
configured static-token scopes (default `lab:read lab:admin`). Every
allowlisted identity, not only `LABBY_AUTH_ADMIN_EMAIL`, could therefore call
`requires_admin` actions. The most serious ones:

- `setup` `draft.set` / `draft.commit` and `settings.env.update`, which write
  the live `$LABBY_HOME/.env`. They can change `LABBY_MCP_HTTP_TOKEN`,
  `LABBY_AUTH_ADMIN_EMAIL`, `LABBY_GOOGLE_CLIENT_SECRET`, allowlist settings,
  and upstream settings, and the changes take effect at the next restart;
- `snippets` execution and `server_logs` queries, which take effect
  immediately.

The fix changes admission from the deploy onward. It does not undo anything
written before the deploy. The exposure window runs from the first allowlist
entry other than the configured admin until the fixed build is running on the
host. See the [browser session scopes](./OAUTH.md#browser-session-scopes-and-domain-admission)
for the current behavior.

## 1. Preserve evidence first

Before changing anything:

1. Snapshot the container (for Incus: `incus snapshot create <container> <name>`).
2. Take SQLite-consistent copies of `$LABBY_HOME/access.db` and
   `$LABBY_HOME/auth.db` (online backup API or `VACUUM INTO`, or stop the service
   and copy each database with its WAL). Do not copy a live database file alone;
   see [OPERATIONS.md](../OPERATIONS.md#oauth-auth-state) and
   [MIGRATION.md](../access-control/MIGRATION.md).
3. Copy `$LABBY_HOME/.env`, `$LABBY_HOME/config.toml`, and every
   `.env.bak.*` and `config.toml.bak.*` file, preserving mode `0600`.
4. Export the service journal for the window.

Treat these copies as credential material.

## 2. Review configuration for tampering

Diff the live `.env` and `config.toml` against the newest copy you know is
good (programmatic writes leave `.env.bak.<timestamp>` and
`config.toml.bak.<nanos>.<pid>.<counter>` files; hand-made backups also count).
Check at least:

- `LABBY_AUTH_ADMIN_EMAIL`, `LABBY_AUTH_ALLOWED_EMAIL_DOMAINS`,
  `LABBY_AUTH_VIEWER_EMAIL_DOMAINS`, and the allowlist entries
  (`GET /v1/auth/allowed-emails`);
- `LABBY_MCP_HTTP_TOKEN`, `LABBY_GOOGLE_CLIENT_ID`, `LABBY_GOOGLE_CLIENT_SECRET`,
  and any Authelia client settings;
- `LABBY_AUTH_MODE`, `LABBY_PUBLIC_URL`, redirect-URI and registration settings;
- upstream entries (especially stdio commands and upstream bearer tokens),
  protected routes, and saved snippets.

Any change you cannot attribute to the operator is a compromise indicator.

## 3. Review the audit trail

The access store records access mutations only. Configuration writes through
`setup` are not in `access_audit`; look for them in the service logs.

On a read-only copy of `access.db` (for example `sqlite3 -readonly copy.db`):

```sql
-- Access mutations in the window (occurred_at is Unix seconds).
SELECT occurred_at, actor_principal_id, action, target_kind, decision, reason_code
  FROM access_audit
 WHERE occurred_at BETWEEN :start AND :end
 ORDER BY occurred_at;

-- Proof and product-credential security events.
SELECT occurred_at, event_kind, decision, reason_code
  FROM access_security_events
 WHERE occurred_at BETWEEN :start AND :end
 ORDER BY occurred_at;
```

Look for `access.platform_admin.grant`, Team membership or role changes,
invitations, and credential issue events that the owner did not make.

In the service journal, look for `setup` actions (`draft.set`,
`draft.commit`, `settings.env.update`), `snippets` execution, and
`server_logs` queries made by browser sessions other than the configured
admin.

## 4. Rotate credentials

Rotate every credential an admin-scoped caller could read or replace:

- `LABBY_MCP_HTTP_TOKEN` (update every automation client afterwards);
- the Google OAuth client secret (`LABBY_GOOGLE_CLIENT_SECRET`) at the
  provider, then in `.env`; likewise the Authelia client secret if used;
- the JWT signing key (`LABBY_AUTH_KEY_PATH`, default `~/.labby/auth-jwt.pem`).
  Replacing it invalidates every issued Labby access token, so it is a global
  session reset; see [OPERATIONS.md](../OPERATIONS.md#oauth-auth-state);
- Depot tokens and every upstream bearer or API key stored in `.env`.

Do not rotate `LABBY_TOKEN_ENCRYPTION_KEY` as part of this step without a plan:
without the old key, persisted provider credentials cannot be decrypted.

Rotate with the service's own `.env`. Note that `just mcp-token` edits `./.env`
in the repository checkout, not `$LABBY_HOME/.env` on the host or in the
container, and it prints the new token to stdout (shell history, CI logs).
Do not use it for production rotation.

## 5. Invalidate sessions and re-verify the owner

1. Remove, then re-add if still wanted, each allowlist entry
   (`DELETE /v1/auth/allowed-emails/{email}`). Removal revokes that email's
   browser sessions, refresh grants, pending codes, and provider credentials.
   These routes accept only the configured admin's browser session.
2. Confirm the fixed build is running and that a non-owner allowlisted session
   no longer holds `lab:admin`.
3. Sign in as the owner and check `GET /auth/session`: `authority_state` is
   `ready`, and `capabilities` contains `platform.manage`.
4. Review who holds platform administration (the `platform_administrators`
   table on the copy; the `access` service has no list action for it) and
   revoke unexpected grants with `access.platform_admin.revoke`.

If the owner identity itself was changed or lost, follow
[Owner recovery](../services/ACCESS.md#owner-recovery).

## Config rollback

`labby incus sync --rollback` restores the binary, web assets, and service
unit state only. It does not restore `config.toml` or `.env`. Labby has no
config-restore command today. To roll back configuration by hand:

1. Stop the service.
2. Copy the chosen `config.toml.bak.*` (and `.env` backup) over the live file.
   Keep owner-only permissions (`0600`) and the original owner.
3. Start the service and check `/ready` and `labby doctor`. Startup is the
   only full config validation today; `labby setup check` does not parse
   `config.toml`.
4. Keep the replaced file for review instead of deleting it.

## Related docs

- [OAuth runtime](./OAUTH.md)
- [Access service](../services/ACCESS.md)
- [Durable-state disaster recovery](./DISASTER_RECOVERY.md)
- [Incus deployment](./INCUS.md)
- [Access-control threat model](../access-control/THREAT_MODEL.md)
