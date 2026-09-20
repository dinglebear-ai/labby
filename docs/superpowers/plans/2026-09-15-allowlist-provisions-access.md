# Allowlist Provisions Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An admin adds a coworker's email (with a role) in Settings → Authentication; the coworker signs in with Google and lands in the app with that access. No links, no CLI, no second step.

**Architecture:** Each allowlist row carries a role (`member` or `admin`). On `GET /auth/session`, when the durable authority store reports the identity as `unprovisioned`, the handler checks the identity's provider-verified email against the allowlist (and the configured `LABBY_AUTH_ADMIN_EMAIL` list, which counts as `admin`). On a match, a new access-store function creates the Principal, an Initial Team membership, a default-Project membership, and (for `admin`) a platform-administrator grant, all in one transaction, then the session is re-resolved as `ready`. The browser never decides admission; it only sees the result.

**Tech Stack:** Rust (axum, rusqlite, tokio), `labby-auth` (allowlist SQLite store), `labby` (`access` store/runtime, `browser_session.rs`), Next.js 16 + React 19 web UI with Aurora primitives, `node:test` + `tsx` for web tests, `cargo nextest` for Rust.

## Global Constraints

- Branch off `feat/multi-provider-auth-admins` (PR #659) — this plan uses `config.admin_emails: Vec<String>` from that branch.
- Never log raw emails or subjects; use `labby_auth::util::fingerprint` where a diagnostic is needed (existing convention in `auth_admin.rs`).
- Roles accepted on the allowlist: exactly `member` and `admin`. Never `owner`.
- Provisioning must be idempotent: a second sign-in returns `AlreadyActive` and changes nothing.
- Provisioning must never upgrade an existing membership's role; changing a role after the fact is still `access.team.member.role.set`.
- Web UI: reuse `@/components/ui/select`, existing `AllowedUsersPanel` styling; no new primitives.
- Rust verification: `cargo nextest run -p labby-auth --all-features` and focused `labby` tests; `cargo clippy --all-features --all-targets -- -D warnings`; `cargo fmt`.
- Web verification: `pnpm exec tsx --test <file>`, `pnpm lint`, `pnpm test:unit`, `pnpm build` from `apps/gateway-admin`.
- Set `CARGO_TARGET_DIR` to a scratch dir and `TMPDIR=/private/tmp/lwt` (create it) for every cargo command, per project memory.

---

## File Map

| File | Responsibility |
| --- | --- |
| `crates/labby-auth/src/types.rs` | `AllowedUserRow` gains `role: String` |
| `crates/labby-auth/src/sqlite.rs` (open path, ~line 1371) | add `role` column to `allowed_users` idempotently |
| `crates/labby-auth/src/sqlite/rows.rs` | map the new column |
| `crates/labby-auth/src/sqlite/allowlist.rs` | `add_allowed_user` takes a role; new `find_allowed_user(email)`; `list` returns role |
| `crates/labby-auth/src/sqlite/tests.rs` | store tests |
| `crates/labby/src/access/team_provision.rs` | new `provision_allowlisted(connection, identity, role)` |
| `crates/labby/src/access/store.rs`, `runtime.rs`, `access.rs` | thin wrappers + export `AllowlistRole` |
| `crates/labby/src/api/browser_session.rs` | admit-on-session logic |
| `crates/labby/src/api/services/auth_admin.rs` | accept/return `role` |
| `crates/labby/tests/auth_admin_api.rs` | API tests |
| `apps/gateway-admin/lib/api/auth-admin-client.ts` | `role` in types/calls |
| `apps/gateway-admin/components/allowed-users-panel.tsx` | role select + column |
| `docs/services/ACCESS.md`, `docs/runtime/OAUTH.md` | describe new onboarding |

---

### Task 1: Allowlist rows carry a role (labby-auth store)

**Files:**
- Modify: `crates/labby-auth/src/types.rs:675-679`
- Modify: `crates/labby-auth/src/sqlite.rs:1364-1395`
- Modify: `crates/labby-auth/src/sqlite/rows.rs:9-15`
- Modify: `crates/labby-auth/src/sqlite/allowlist.rs`
- Test: `crates/labby-auth/src/sqlite/tests.rs`

**Interfaces:**
- Produces: `AllowedUserRow { email, added_by, created_at, role: String }`; `SqliteStore::add_allowed_user(&self, email: &str, added_by: &str, role: &str, created_at: i64) -> Result<(), AuthError>`; `SqliteStore::find_allowed_user(&self, email: &str) -> Result<Option<AllowedUserRow>, AuthError>` (case-insensitive).

- [ ] **Step 1: Write the failing store test**

Append to `crates/labby-auth/src/sqlite/tests.rs` (before the `_assert_allowed_user_row_type` helper):

```rust
#[tokio::test]
async fn allowlist_rows_store_a_role_and_are_found_case_insensitively() {
    let store = temp_store().await;
    store
        .add_allowed_user("Eli@Example.com", "owner-sub", "admin", 42)
        .await
        .unwrap();
    store
        .add_allowed_user("bob@example.com", "owner-sub", "member", 43)
        .await
        .unwrap();
    let eli = store.find_allowed_user("ELI@example.com").await.unwrap().unwrap();
    assert_eq!(eli.email, "eli@example.com");
    assert_eq!(eli.role, "admin");
    assert!(store.find_allowed_user("nobody@example.com").await.unwrap().is_none());
    let listed = store.list_allowed_users().await.unwrap();
    assert_eq!(
        listed.iter().map(|row| row.role.as_str()).collect::<Vec<_>>(),
        vec!["admin", "member"]
    );
    assert!(matches!(
        store.add_allowed_user("x@example.com", "owner-sub", "owner", 44).await,
        Err(crate::error::AuthError::Validation(_))
    ));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p labby-auth --all-features -E 'test(allowlist_rows_store_a_role)'`
Expected: compile error — `add_allowed_user` takes 3 args, no `find_allowed_user`, no `role` field.

- [ ] **Step 3: Add the field and column**

`crates/labby-auth/src/types.rs` — replace the struct:

```rust
pub struct AllowedUserRow {
    pub email: String,
    pub added_by: String,
    pub created_at: i64,
    /// Access granted at first sign-in: `member` or `admin`.
    pub role: String,
}
```

`crates/labby-auth/src/sqlite.rs` — right after the existing `add_column_if_missing(&conn, "refresh_tokens", "resource", ...)?;` call (~line 1385) add:

```rust
    add_column_if_missing(
        &conn,
        "allowed_users",
        "role",
        "TEXT NOT NULL DEFAULT 'member'",
    )?;
```

`crates/labby-auth/src/sqlite/rows.rs`:

```rust
pub(super) fn row_to_allowed_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<AllowedUserRow> {
    Ok(AllowedUserRow {
        email: row.get(0)?,
        added_by: row.get(1)?,
        created_at: row.get(2)?,
        role: row.get(3)?,
    })
}
```

- [ ] **Step 4: Update the allowlist store functions**

In `crates/labby-auth/src/sqlite/allowlist.rs`, replace `add_allowed_user` and `list_allowed_users`, and add `find_allowed_user`:

```rust
    /// Roles an allowlist entry may grant at first sign-in.
    pub const ALLOWED_USER_ROLES: [&'static str; 2] = ["member", "admin"];

    /// Add an email address to the allowlist with the access it receives at
    /// first sign-in.
    ///
    /// `email` is normalised to lowercase before storage. Returns
    /// `AuthError::Validation` if the email is already present or `role` is
    /// not one of [`Self::ALLOWED_USER_ROLES`].
    pub async fn add_allowed_user(
        &self,
        email: &str,
        added_by: &str,
        role: &str,
        created_at: i64,
    ) -> Result<(), AuthError> {
        if !Self::ALLOWED_USER_ROLES.contains(&role) {
            return Err(AuthError::Validation(
                "allowlist role must be `member` or `admin`".into(),
            ));
        }
        let email = email.to_lowercase();
        let fp = fingerprint(&email);
        let added_by = added_by.to_string();
        let role = role.to_string();
        self.with_conn(move |conn| {
            let changed = conn
                .execute(
                    "INSERT INTO allowed_users (email, added_by, created_at, role)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![email, added_by, created_at, role],
                )
                .map_err(|error| match error {
                    rusqlite::Error::SqliteFailure(ref e, _)
                        if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        AuthError::Validation(format!(
                            "email fingerprint {fp} is already in the allowlist"
                        ))
                    }
                    other => sqlite_error(other),
                })?;
            debug_assert_eq!(changed, 1);
            Ok(())
        })
        .await
    }

    /// Return the allowlist row for `email` (case-insensitive), if any.
    pub async fn find_allowed_user(&self, email: &str) -> Result<Option<AllowedUserRow>, AuthError> {
        let email = email.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT email, added_by, created_at, role
                   FROM allowed_users WHERE email = ?1 COLLATE NOCASE",
                params![email],
                row_to_allowed_user,
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Return all allowlist rows ordered by `created_at ASC`.
    pub async fn list_allowed_users(&self) -> Result<Vec<AllowedUserRow>, AuthError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT email, added_by, created_at, role
                     FROM allowed_users
                     ORDER BY created_at ASC",
                )
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map([], row_to_allowed_user)
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            Ok(rows)
        })
        .await
    }
```

Add `use rusqlite::OptionalExtension;` to the file's imports.

- [ ] **Step 5: Fix every caller of `add_allowed_user` / `AllowedUserRow`**

Run `rg -n "add_allowed_user\(|AllowedUserRow \{" crates --type rust`. For each test call `add_allowed_user(email, by, ts)` insert `"member",` before the timestamp. For each `AllowedUserRow { .. }` literal add `role: "member".into(),`. Known sites: `crates/labby-auth/src/authorize.rs`, `crates/labby-auth/src/state.rs` tests, `crates/labby-auth/src/sqlite/tests.rs`, `crates/labby/src/api/router.rs` (`colleague_and_admin_auth_state`, `every_listed_admin_passes_...`), `crates/labby/src/api/services/auth_admin.rs` (leave the handler for Task 5 — just make it compile with `"member"` for now), `crates/labby/tests/auth_admin_api.rs`.

- [ ] **Step 6: Run the crate suites**

Run: `cargo nextest run -p labby-auth --all-features` then `cargo check -p labby --all-features --tests`
Expected: all pass / compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/labby-auth crates/labby
git commit -m "feat(auth): allowlist entries carry a member or admin role"
```

---

### Task 2: Access store provisions an allowlisted identity

**Files:**
- Modify: `crates/labby/src/access/team_provision.rs`
- Modify: `crates/labby/src/access/store.rs:1321-1345`
- Modify: `crates/labby/src/access/runtime.rs:227-260`
- Modify: `crates/labby/src/access.rs:190`
- Test: `crates/labby/src/access/team_provision.rs` (tests module)

**Interfaces:**
- Produces: `pub(crate) enum AllowlistRole { Member, Admin }` with `AllowlistRole::parse(&str) -> Option<Self>`; `AccessStore::provision_allowlisted(&self, identity: VerifiedIdentity, role: AllowlistRole) -> AccessStoreResult<TeamMemberProvisionOutcome>`; `AccessRuntime::provision_allowlisted(&self, identity: VerifiedIdentity, role: AllowlistRole) -> Result<TeamMemberProvisionOutcome, AccessRuntimeError>`.

- [ ] **Step 1: Write the failing store test**

Add to the `tests` module at the bottom of `crates/labby/src/access/team_provision.rs`:

```rust
    async fn membership_rows(store: &AccessStore) -> (Vec<(String, String)>, Vec<(String, String)>, i64) {
        store
            .with_connection(|connection| {
                let mut teams = connection
                    .prepare("SELECT team_id, role FROM team_memberships WHERE principal_id LIKE 'team-member-%' ORDER BY team_id")
                    .map_err(map_sqlite_error)?;
                let teams = teams
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(map_sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(map_sqlite_error)?;
                let mut projects = connection
                    .prepare("SELECT project_id, role FROM project_memberships WHERE principal_id LIKE 'team-member-%' ORDER BY project_id")
                    .map_err(map_sqlite_error)?;
                let projects = projects
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(map_sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(map_sqlite_error)?;
                let admins: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM platform_administrators WHERE status='active' AND principal_id LIKE 'team-member-%'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite_error)?;
                Ok((teams, projects, admins))
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn allowlisted_member_gets_team_and_project_membership_once() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli");
        assert_eq!(
            store.provision_allowlisted(eli.clone(), AllowlistRole::Member).await.unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(teams, vec![("bootstrap-initial-team".to_owned(), "member".to_owned())]);
        assert_eq!(projects, vec![("bootstrap-default".to_owned(), "member".to_owned())]);
        assert_eq!(admins, 0);
        // Repeat sign-in is a no-op and never upgrades the role.
        assert_eq!(
            store.provision_allowlisted(eli.clone(), AllowlistRole::Admin).await.unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        assert_eq!(membership_rows(&store).await, (teams, projects, 0));
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(!snapshot.platform_administrator);
    }

    #[tokio::test]
    async fn allowlisted_admin_is_team_admin_and_platform_admin() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli-admin");
        assert_eq!(
            store.provision_allowlisted(eli.clone(), AllowlistRole::Admin).await.unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(teams, vec![("bootstrap-initial-team".to_owned(), "admin".to_owned())]);
        assert_eq!(projects, vec![("bootstrap-default".to_owned(), "admin".to_owned())]);
        assert_eq!(admins, 1);
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(snapshot.platform_administrator);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p labby --all-features -E 'test(allowlisted_)'`
Expected: compile error — `AllowlistRole` / `provision_allowlisted` undefined.

- [ ] **Step 3: Implement `provision_allowlisted`**

In `crates/labby/src/access/team_provision.rs`, add after `provision_viewer`:

```rust
const INITIAL_TEAM_ID: &str = "bootstrap-initial-team";

/// Access an allowlist entry grants at first sign-in. Product-owned admission
/// only; callers cannot request `owner`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllowlistRole {
    Member,
    Admin,
}

impl AllowlistRole {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "member" => Some(Self::Member),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
        }
    }
}

/// Admit an allowlisted identity: a Principal (if missing), an Initial Team
/// membership, a default-Project membership, and for `Admin` a platform
/// administrator grant — one transaction, idempotent. An existing active
/// Project membership means the identity was already admitted; nothing is
/// upgraded and `AlreadyActive` is returned.
pub(super) fn provision_allowlisted(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    role: AllowlistRole,
) -> AccessStoreResult<TeamMemberProvisionOutcome> {
    let project_id = super::bootstrap::PROJECT_ID;
    let outcome = provision_with_role(
        connection,
        identity,
        project_id,
        match role {
            AllowlistRole::Member => InitialRole::Member,
            AllowlistRole::Admin => InitialRole::Admin,
        },
    )?;
    if outcome == TeamMemberProvisionOutcome::AlreadyActive {
        return Ok(outcome);
    }
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let (principal_id, organization_id): (String, String) = transaction
        .query_row(
            "SELECT p.principal_id,p.organization_id FROM principal_links l
             JOIN principals p ON p.principal_id=l.principal_id
             WHERE l.link_kind='external' AND l.issuer=?1 AND l.subject=?2 AND l.status='active'",
            params![issuer, subject],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AccessStoreError::MalformedVocabulary)?
            .as_secs(),
    )
    .unwrap_or(i64::MAX);
    transaction
        .execute(
            "INSERT OR IGNORE INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at)
             VALUES(?1,?2,?3,?4,?5,'active',1,?4,?6,?6,NULL)",
            params![
                format!("team-member-{INITIAL_TEAM_ID}-{principal_id}"),
                organization_id,
                INITIAL_TEAM_ID,
                principal_id,
                role.as_str(),
                now
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "UPDATE groups SET membership_epoch=membership_epoch+1,updated_at=?1
             WHERE organization_id=?2 AND group_id=?3 AND status!='deleted'",
            params![now, organization_id, INITIAL_TEAM_ID],
        )
        .map_err(map_sqlite_error)?;
    if role == AllowlistRole::Admin {
        transaction
            .execute(
                "INSERT INTO platform_administrators(principal_id,status,authority_epoch,granted_by,created_at,updated_at,revoked_at)
                 VALUES(?1,'active',1,?1,?2,?2,NULL)
                 ON CONFLICT(principal_id) DO UPDATE SET status='active',authority_epoch=platform_administrators.authority_epoch+1,updated_at=excluded.updated_at,revoked_at=NULL",
                params![principal_id, now],
            )
            .map_err(map_sqlite_error)?;
    }
    transaction
        .execute(
            "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1",
            [now],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
             VALUES(?1,?2,NULL,?3,?4,?5,'access.allowlist.provision','team_membership',?6,'allow','allowlist_admission',1,?7)",
            params![
                format!("allowlist-provision-{}", identity.safe_fingerprint().replace(':', "-")),
                now,
                principal_id,
                organization_id,
                project_id,
                format!("{INITIAL_TEAM_ID}\0{principal_id}"),
                serde_json::json!({"role": role.as_str()}).to_string()
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(TeamMemberProvisionOutcome::Created)
}
```

Extend `InitialRole` in the same file:

```rust
enum InitialRole {
    Member,
    Viewer,
    Admin,
}

impl InitialRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Viewer => "viewer",
            Self::Admin => "admin",
        }
    }
}
```

- [ ] **Step 4: Wire store, runtime, and exports**

`crates/labby/src/access/store.rs` — after `provision_team_viewer`:

```rust
    /// Called only after the session handler has matched the identity's
    /// provider-verified email against the allowlist or configured admins.
    pub(crate) async fn provision_allowlisted(
        &self,
        identity: labby_auth::VerifiedIdentity,
        role: super::AllowlistRole,
    ) -> AccessStoreResult<super::TeamMemberProvisionOutcome> {
        self.with_connection(move |connection| {
            super::team_provision::provision_allowlisted(connection, &identity, role)
        })
        .await
    }
```

`crates/labby/src/access/runtime.rs` — after `provision_team_viewer`:

```rust
    pub(crate) async fn provision_allowlisted(
        &self,
        identity: labby_auth::VerifiedIdentity,
        role: super::AllowlistRole,
    ) -> Result<super::TeamMemberProvisionOutcome, AccessRuntimeError> {
        let _writer = self.acquire_bootstrap_writer().await?;
        self.security_store()
            .await?
            .provision_allowlisted(identity, role)
            .await
            .map_err(|_| AccessRuntimeError::LifecycleUnavailable)
    }
```

`crates/labby/src/access.rs` line 190: change to
`pub(crate) use team_provision::{AllowlistRole, TeamMemberProvisionOutcome};`

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p labby --all-features -E 'test(/team_provision/)'`
Expected: all pass, including the two new tests and the existing viewer tests.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/access
git commit -m "feat(access): provision an allowlisted identity with a member or admin role"
```

---

### Task 3: `/auth/session` admits allowlisted identities

**Files:**
- Modify: `crates/labby/src/api/browser_session.rs:590-608` (`project_session`)
- Test: `crates/labby/src/api/browser_session.rs` (tests module)

**Interfaces:**
- Consumes: `AccessRuntime::provision_allowlisted`, `AllowlistRole::parse`, `SqliteStore::find_allowed_user`, `SqliteStore::current_verified_inbound_email(issuer, subject)`, `AuthConfig::is_admin_email`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module of `crates/labby/src/api/browser_session.rs`:

```rust
    /// An allowlisted identity is admitted on its first `/auth/session`:
    /// the durable authority is created and the same call projects `ready`.
    #[tokio::test]
    async fn allowlisted_session_is_provisioned_on_first_session_read() {
        let directory = tempfile::Builder::new()
            .prefix("labby-allowlist-admission-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let owner = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        let runtime = std::sync::Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        );
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(owner, "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        let auth_config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth-jwt.pem"),
            admin_emails: vec!["owner@example.com".into()],
            google: labby_auth::config::GoogleConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                callback_url: None,
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into()],
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        let auth_state = labby_auth::state::AuthState::new(auth_config.clone()).await.unwrap();
        let binding = auth_state.inbound_provider_binding();
        auth_state
            .store
            .add_allowed_user("eli@example.com", "owner-sub", "admin", 1)
            .await
            .unwrap();
        for (subject, email) in [("eli-sub", "eli@example.com"), ("stranger-sub", "stranger@example.com")] {
            auth_state
                .store
                .upsert_bound_verified_inbound_identity(subject, email, 2, binding.clone())
                .await
                .unwrap();
        }
        let state = AppState::new()
            .with_access_runtime(runtime)
            .with_auth_config(auth_config)
            .with_oauth_state(auth_state);
        let caller = |subject: &str, email: &str| SessionCaller {
            identity: labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                "https://accounts.google.com",
                subject,
            )
            .unwrap(),
            via_session: true,
            subject: subject.into(),
            email: Some(email.into()),
            scopes: vec!["lab:read".into(), "lab".into()],
            transport_admin: false,
        };
        let view = |sub: &str| SessionView {
            login_available: true,
            user: SessionUser { sub: sub.to_owned(), email: None },
            project_id: None,
            expires_at: 1,
            csrf_token: String::new(),
        };

        let body = project_session(&state, caller("eli-sub", "eli@example.com"), view("eli-sub"))
            .await
            .unwrap();
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(body["is_admin"], true, "allowlist role admin grants platform.manage");

        // Display email is not evidence: a session claiming an allowlisted
        // email whose verified identity is someone else stays unprovisioned.
        let body = project_session(&state, caller("stranger-sub", "eli@example.com"), view("stranger-sub"))
            .await
            .unwrap();
        assert_eq!(body["authority_state"], "unprovisioned");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p labby --all-features -E 'test(allowlisted_session_is_provisioned)'`
Expected: FAIL — first assertion sees `"unprovisioned"`.

- [ ] **Step 3: Implement admission in `project_session`**

Replace the body of `project_session` in `crates/labby/src/api/browser_session.rs`:

```rust
async fn project_session(
    state: &AppState,
    caller: SessionCaller,
    view: SessionView,
) -> Result<serde_json::Value, ToolError> {
    let admitted = crate::access::owner_bootstrap_admission(
        &caller.bootstrap_caller(),
        state
            .auth_config
            .as_ref()
            .map(|config| config.admin_emails.as_slice()),
    )
    .is_ok();
    let mut authority =
        resolve_session_authority(state, caller.identity.clone(), caller.transport_admin).await?;
    if matches!(authority, SessionAuthority::Unprovisioned)
        && admit_allowlisted_identity(state, &caller).await?
    {
        authority =
            resolve_session_authority(state, caller.identity, caller.transport_admin).await?;
    }
    let owner_bootstrap_available = admitted
        && state.access_runtime.owner_bootstrap_offer().await == OwnerBootstrapOffer::Available;
    authenticated_session_body(&view, &authority, owner_bootstrap_available)
}

/// Durable admission for a browser session whose provider-verified email is
/// on the allowlist or in `LABBY_AUTH_ADMIN_EMAIL`. Returns `true` when a
/// Principal was created or already existed, so the caller re-resolves
/// authority. The session's display email is never consulted: evidence comes
/// from the provider-verified identity row bound to this issuer and subject.
async fn admit_allowlisted_identity(
    state: &AppState,
    caller: &SessionCaller,
) -> Result<bool, ToolError> {
    let (Some(auth_state), Some(config)) = (oauth_state(state), state.auth_config.as_ref()) else {
        return Ok(false);
    };
    if !caller.via_session {
        return Ok(false);
    }
    let labby_auth::PrincipalLink::External { issuer, subject } = caller.identity.principal_link()
    else {
        return Ok(false);
    };
    let Some(email) = auth_state
        .store
        .current_verified_inbound_email(issuer, subject)
        .await
        .map_err(|_| ToolError::internal_message("verified identity lookup failed"))?
    else {
        return Ok(false);
    };
    let role = if config.is_admin_email(&email) {
        Some(crate::access::AllowlistRole::Admin)
    } else {
        auth_state
            .store
            .find_allowed_user(&email)
            .await
            .map_err(|_| ToolError::internal_message("allowlist lookup failed"))?
            .and_then(|row| crate::access::AllowlistRole::parse(&row.role))
    };
    let Some(role) = role else {
        return Ok(false);
    };
    match state
        .access_runtime
        .provision_allowlisted(caller.identity.clone(), role)
        .await
    {
        Ok(_) => Ok(true),
        Err(error) => {
            // Fail closed to `unprovisioned`, but leave a trace; identity and
            // email are never logged.
            tracing::warn!(
                surface = "api",
                service = "auth",
                action = "session.get",
                error = %error,
                "allowlist admission failed; session stays unprovisioned"
            );
            Ok(false)
        }
    }
}
```

`SessionCaller` must derive `Clone` on `identity` usage: `caller.identity.clone()` requires `VerifiedIdentity: Clone` (it is). If `project_session` moved `caller.identity` before, the first `resolve_session_authority` call now clones; the second consumes.

- [ ] **Step 4: Run the tests**

Run: `cargo nextest run -p labby --all-features -E 'test(/browser_session/)'`
Expected: all pass, including `session_authority_states_are_explicit` (the unprovisioned stranger there has no verified email row, so it stays unprovisioned).

- [ ] **Step 5: Commit**

```bash
git add crates/labby/src/api/browser_session.rs
git commit -m "feat(auth): admit allowlisted identities on first session read"
```

---

### Task 4: Allowlist API accepts and returns the role

**Files:**
- Modify: `crates/labby/src/api/services/auth_admin.rs:255-370`
- Modify: `crates/labby/src/api/openapi.rs` (the `allowed-emails` request/response schema, find with `rg -n "allowed-emails" crates/labby/src/api/openapi.rs`)
- Test: `crates/labby/tests/auth_admin_api.rs`

**Interfaces:**
- Produces: `POST /v1/auth/allowed-emails` body `{ "email": string, "role"?: "member" | "admin" }` (default `member`); every entry in `GET` and the `POST` response includes `"role"`.

- [ ] **Step 1: Write the failing API tests**

Add to `crates/labby/tests/auth_admin_api.rs`:

```rust
#[tokio::test]
async fn post_admin_session_adds_email_with_role_and_lists_it() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let app = h.router();
    let response = app
        .clone()
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"eli@example.com","role":"admin"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["entry"]["role"], "admin");

    let response = app
        .oneshot(Harness::get_with_session("/v1/auth/allowed-emails", &session))
        .await
        .unwrap();
    let json = body_json(response).await;
    let entry = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["email"] == "eli@example.com")
        .unwrap();
    assert_eq!(entry["role"], "admin");
}

#[tokio::test]
async fn post_without_role_defaults_to_member() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let response = h
        .router()
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"bob@example.com"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(body_json(response).await["entry"]["role"], "member");
}

#[tokio::test]
async fn post_owner_role_returns_422() {
    let h = Harness::new().await;
    let session = h.seed_admin_session().await;
    let response = h
        .router()
        .oneshot(Harness::post_with_session(
            "/v1/auth/allowed-emails",
            &session,
            r#"{"email":"bob@example.com","role":"owner"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run -p labby --all-features --test auth_admin_api -E 'test(/role/)'`
Expected: `role` is `null` in the response / 201 for `owner`.

- [ ] **Step 3: Implement**

In `crates/labby/src/api/services/auth_admin.rs`:

```rust
#[derive(Deserialize)]
struct AddEmailBody {
    email: String,
    #[serde(default = "default_allowlist_role")]
    role: String,
}

fn default_allowlist_role() -> String {
    "member".to_string()
}

fn validate_role(raw: &str) -> Result<&str, ToolError> {
    let role = raw.trim();
    if labby_auth::sqlite::SqliteStore::ALLOWED_USER_ROLES.contains(&role) {
        Ok(role)
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: "role must be `member` or `admin`".to_string(),
        })
    }
}
```

In `add_allowed_email`, directly after the email validation block:

```rust
    let role = match validate_role(&body.role) {
        Ok(role) => role.to_owned(),
        Err(err) => {
            log_auth_dispatch(action, req_id.as_deref(), start, Some(err.kind()), actor_key);
            return no_store(ApiError::new(err).into_response());
        }
    };
```

Change the store call to `.add_allowed_user(&email, &added_by, &role, created_at)` and the response entry to include `role: role.clone()`. Update the module doc comment on the `POST` route to `Body: { "email": ..., "role": "member" | "admin" }`.

In `openapi.rs`, add `role` (string enum `member`,`admin`, default `member`) to the add-email request schema and to the entry schema. Run `just docs-generate` afterwards so `docs/generated/openapi.json` and `api-routes.*` refresh.

- [ ] **Step 4: Run the API suite, docs check**

Run: `cargo nextest run -p labby --all-features --test auth_admin_api` then `just docs-generate && just docs-check`
Expected: all pass; docs check OK.

- [ ] **Step 5: Commit**

```bash
git add crates/labby/src/api docs/generated crates/labby/tests/auth_admin_api.rs
git commit -m "feat(api): allowlist entries accept and report a role"
```

---

### Task 5: Web UI — role select and column

**Files:**
- Modify: `apps/gateway-admin/lib/api/auth-admin-client.ts`
- Modify: `apps/gateway-admin/components/allowed-users-panel.tsx`
- Test: `apps/gateway-admin/lib/api/auth-admin-client.test.ts`, `apps/gateway-admin/components/allowed-users-panel.test.tsx`

**Interfaces:**
- Consumes: API from Task 4.
- Produces: `authAdminApi.addAllowedEmail(email: string, role: AllowedEmailRole, signal?)`; `AllowedEmailEntry.role: AllowedEmailRole`.

- [ ] **Step 1: Write the failing tests**

Append to `apps/gateway-admin/lib/api/auth-admin-client.test.ts` (follow the file's existing fetch-mock pattern — open it first and copy the helper it uses):

```ts
test('addAllowedEmail posts the role', async () => {
  let sentBody = ''
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    sentBody = String(init?.body)
    return new Response(JSON.stringify({ entry: { email: 'eli@example.com', added_by: 'owner', created_at: '1', role: 'admin' } }), { status: 201 })
  }) as typeof globalThis.fetch
  const entry = await authAdminApi.addAllowedEmail('eli@example.com', 'admin')
  assert.deepEqual(JSON.parse(sentBody), { email: 'eli@example.com', role: 'admin' })
  assert.equal(entry.role, 'admin')
})
```

Append to `apps/gateway-admin/components/allowed-users-panel.test.tsx`:

```tsx
test('AllowedUsersPanel offers a role choice defaulting to member', () => {
  const markup = render()
  assert.match(markup, /Role for new user/)
  assert.match(markup, /Member/)
})
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/api/auth-admin-client.test.ts components/allowed-users-panel.test.tsx`
Expected: FAIL (type error on the second argument / missing "Role for new user").

- [ ] **Step 3: Update the client**

`apps/gateway-admin/lib/api/auth-admin-client.ts`:

```ts
export type AllowedEmailRole = 'member' | 'admin'

export interface AllowedEmailEntry {
  email: string
  added_by: string
  created_at: string
  role: AllowedEmailRole
}
```

and

```ts
  async addAllowedEmail(
    email: string,
    role: AllowedEmailRole,
    signal?: AbortSignal,
  ): Promise<AllowedEmailEntry> {
    const data = await apiFetch<{ entry: AllowedEmailEntry }>(
      '/auth/allowed-emails',
      {
        method: 'POST',
        body: JSON.stringify({ email, role }),
        signal,
      },
    )
    return data.entry
  },
```

- [ ] **Step 4: Update the panel**

In `apps/gateway-admin/components/allowed-users-panel.tsx`:

Imports:

```ts
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  authAdminApi,
  AuthAdminApiError,
  type AllowedEmailEntry,
  type AllowedEmailRole,
} from '@/lib/api/auth-admin-client'
```

State, next to `addEmail`:

```ts
  const [addRole, setAddRole] = useState<AllowedEmailRole>('member')
```

In `handleAdd`, replace the add call and toast:

```ts
      await authAdminApi.addAllowedEmail(email, addRole)
      setAddEmail('')
      setAddRole('member')
      toast.success(`${email} can now sign in as ${addRole}.`)
```

Subtitle copy: replace `Only these email addresses can sign in via OAuth.` with `Anyone listed here can sign in with Google and gets the chosen access right away.`

Add the select between the email input wrapper and the Add button:

```tsx
        <div>
          <label htmlFor="allowed-email-role" className="sr-only">
            Role for new user
          </label>
          <Select value={addRole} onValueChange={(value) => setAddRole(value as AllowedEmailRole)} disabled={isAdding}>
            <SelectTrigger id="allowed-email-role" className="w-[130px]">
              <SelectValue placeholder="Member" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="member">Member</SelectItem>
              <SelectItem value="admin">Admin</SelectItem>
            </SelectContent>
          </Select>
        </div>
```

Table: add a `Role` header after `Email` and a cell `<td className="py-2.5 pr-4 text-aurora-text-muted">{entry.role}</td>` after the email cell.

Confirm-dialog copy: `${pendingRemove.email} will be signed out and can no longer sign in.`

- [ ] **Step 5: Run web checks**

Run from `apps/gateway-admin`:
`pnpm exec tsx --test lib/api/auth-admin-client.test.ts components/allowed-users-panel.test.tsx components/allowed-users-panel-confirmation.test.tsx` → all pass
`pnpm exec tsc --noEmit -p tsconfig.json` → clean
`pnpm lint` → 0 errors
`pnpm test:unit` → all pass
`pnpm build` → succeeds

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin
git commit -m "feat(web): choose member or admin when allowing a sign-in email"
```

---

### Task 6: Docs

**Files:**
- Modify: `docs/services/ACCESS.md:83-130` (Onboard a teammate)
- Modify: `docs/runtime/OAUTH.md` (browser session scopes table row for allowlisted identities, ~line 265)

- [ ] **Step 1: Rewrite the onboarding section**

Replace the numbered list in `docs/services/ACCESS.md` "Onboard a teammate" with:

```markdown
1. **Add the email.** In **Settings → Authentication → Allowed users**, enter
   the teammate's email and choose a role: **Member** (Initial Team member,
   default Project member) or **Admin** (Initial Team admin, default Project
   admin, and platform administrator). API: `POST /v1/auth/allowed-emails`
   with `{"email": "...", "role": "member" | "admin"}`; `role` defaults to
   `member`. Only a configured admin's browser session may do this.
2. **The teammate signs in.** On their first `GET /auth/session` the server
   matches the provider-verified email (never the session's display email)
   against the allowlist and creates the Principal, Team membership, Project
   membership, and any platform-admin grant in one transaction
   (`crates/labby/src/access/team_provision.rs`, `provision_allowlisted`).
   The session projects `ready` immediately. Emails listed in
   `LABBY_AUTH_ADMIN_EMAIL` are admitted as `admin` the same way.
3. **Later changes** use the `access` service: `access.team.member.role.set`,
   `access.platform_admin.grant` / `.revoke`, `access.team.member.remove`.
   Removing the allowlist entry (`DELETE /v1/auth/allowed-emails/:email`)
   signs the identity out and blocks future sign-in; it does not delete the
   Principal.

"No access yet" now only appears for an identity that signed in but is on
neither the allowlist nor the admin list, for example one admitted by
`LABBY_AUTH_ALLOWED_EMAIL_DOMAINS` alone. Add the email with a role to admit
it.
```

Remove the "known product gap" paragraph and the "Learn the `principal_id`" step.

- [ ] **Step 2: Note the scope rule in OAUTH.md**

In the browser session scopes table, keep the transport row as is and add one sentence below the table: `Durable authority is separate: an allowlisted identity is also provisioned into the access store on its first session read with the role chosen when it was allowed (see [Access service](../services/ACCESS.md#onboard-a-teammate-no-access-yet)).`

- [ ] **Step 3: Docs check and commit**

Run: `just docs-check`
Expected: OK

```bash
git add docs/services/ACCESS.md docs/runtime/OAUTH.md
git commit -m "docs(access): allowlisting an email provisions access at first sign-in"
```

---

### Task 7: Full gates and PR

- [ ] **Step 1: Run the gates**

```bash
cargo fmt --all -- --check
cargo clippy -p labby-auth -p labby --all-features --all-targets -- -D warnings
cargo nextest run -p labby-auth --all-features
cargo nextest run -p labby --all-features -E 'test(/access/) | test(/auth/) | test(/browser_session/) | test(/team_provision/) | binary(auth_admin_api)'
just docs-check
cd apps/gateway-admin && pnpm lint && pnpm test:unit && pnpm build
```

Expected: every command exits 0.

- [ ] **Step 2: Push and open the PR against `feat/multi-provider-auth-admins`** (so #659 merges first), title `feat(access): allowlisted emails get access at first sign-in`. Body: goal, the role table, the security note that display email is never used as evidence, and the verification commands above. Do not include `docs/superpowers/plans/` in the PR (protected tree) unless the maintainer applies `protected-docs-approved`.
