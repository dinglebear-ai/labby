//! Identity-issued credentials for an already assigned restricted Tailcat project.
//! The caller holds the published policy lease and runtime writer through commit.
use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use subtle::ConstantTimeEq as _;

use super::{AccessStore, MutationOutcome, error::AccessStoreError, store::map_sqlite_error};

#[derive(Clone)]
pub(crate) struct TailcatEnrollmentInput {
    pub identity: VerifiedIdentity,
    pub installation_id: String,
    pub project_id: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub policy_fingerprint: [u8; 32],
    pub credential_id: String,
    pub credential_digest: [u8; 32],
    pub request_digest: [u8; 32],
    pub idempotency_digest: [u8; 32],
    pub now: i64,
    pub expires_at: i64,
}

impl AccessStore {
    /// Final controller-preference publication fence for an existing enrolled credential.
    /// Policy publication and the prepared host lock must remain held by the caller.
    /// The callback may only perform the bounded final native configuration replacement.
    pub(crate) async fn commit_tailcat_enrolled_configuration<T: Send + 'static>(
        &self,
        input: TailcatEnrollmentInput,
        commit: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, AccessStoreError> {
        if input.identity.authenticator() != Authenticator::BrowserSession {
            return Err(AccessStoreError::NotAuthorized);
        }
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let principal = super::read::resolve_principal(&tx, &input.identity)?;
            let current: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM project_credentials c
                 JOIN project_memberships m ON m.organization_id=c.organization_id AND m.project_id=c.project_id AND m.principal_id=c.principal_id
                 JOIN projects p ON p.organization_id=c.organization_id AND p.project_id=c.project_id
                 JOIN organizations o ON o.organization_id=c.organization_id
                 JOIN project_loadouts l ON l.organization_id=c.organization_id AND l.project_id=c.project_id
                 JOIN project_policy_publications v ON v.project_id=c.project_id
                 JOIN access_installations i ON i.singleton=1
                 JOIN credential_idempotency r ON r.credential_id=c.credential_id AND r.operation='issue'
                 WHERE c.credential_id=?1 AND c.credential_digest=?2 AND c.principal_id=?3 AND c.organization_id=?4
                 AND c.project_id=?5 AND c.loadout_id=?6 AND c.route_id=?6 AND c.resource=?7 AND c.audience=?7
                 AND c.loadout_policy_fingerprint=?8 AND v.policy_fingerprint=?8
                 AND c.installation_id=?9 AND i.installation_id=?9 AND c.installation_generation=i.installation_generation
                 AND c.status='active' AND c.expires_at>?10 AND c.expires_at=?11 AND c.scopes_json='[\"lab\"]'
                 AND m.status='active' AND m.role='owner' AND p.status='active' AND o.status='active'
                 AND l.loadout_name=?6 AND c.membership_generation=m.updated_at AND c.loadout_assignment_generation=l.updated_at
                 AND c.organization_policy_epoch=o.policy_epoch AND c.project_policy_epoch=p.project_policy_epoch
                 AND c.loadout_generation=v.policy_epoch AND c.route_generation=v.policy_epoch AND c.catalog_generation=v.policy_epoch
                 AND r.idempotency_digest=?12 AND r.request_digest=?13 AND r.status='committed'
                 AND EXISTS(SELECT 1 FROM principal_links k WHERE k.credential_id=c.credential_id AND k.principal_id=c.principal_id AND k.link_kind='local_credential' AND k.status='active')
                 AND NOT EXISTS(SELECT 1 FROM access_tombstones t WHERE t.artifact_kind='credential' AND (t.public_id=c.credential_id OR t.canonical_digest=c.credential_digest)))",
                params![input.credential_id,input.credential_digest.as_slice(),principal.id,principal.organization_id,
                    input.project_id,input.loadout_id,input.resource,input.policy_fingerprint.as_slice(),input.installation_id,
                    labby_auth::util::now_unix(),input.expires_at,input.idempotency_digest.as_slice(),input.request_digest.as_slice()],
                |row| row.get(0)
            ).map_err(map_sqlite_error)?;
            if !current { return Err(AccessStoreError::NotAuthorized); }
            let result = commit();
            drop(tx);
            Ok(result)
        }).await
    }

    /// Read-only admission before publishing any secret file; commit rechecks the same facts.
    pub(crate) async fn require_tailcat_enrollment_assignment(
        &self,
        identity: VerifiedIdentity,
        project: String,
        loadout: String,
        fingerprint: [u8; 32],
    ) -> Result<(), AccessStoreError> {
        self.with_connection(move |connection| {
            let tx=connection.transaction_with_behavior(TransactionBehavior::Deferred).map_err(map_sqlite_error)?;
            let principal=super::read::resolve_principal(&tx,&identity)?;
            let current: Option<(String,String)> = tx.query_row("SELECT m.role,l.loadout_name FROM project_memberships m JOIN projects p ON p.organization_id=m.organization_id AND p.project_id=m.project_id JOIN organizations o ON o.organization_id=m.organization_id JOIN project_loadouts l ON l.organization_id=m.organization_id AND l.project_id=m.project_id WHERE m.organization_id=?1 AND m.project_id=?2 AND m.principal_id=?3 AND m.status='active' AND p.status='active' AND o.status='active'",params![principal.organization_id,project,principal.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(map_sqlite_error)?;
            let Some((role,assigned))=current else { return Err(AccessStoreError::ProjectLoadoutConflict); };
            if role!="owner" { return Err(AccessStoreError::NotAuthorized); }
            if assigned!=loadout { return Err(AccessStoreError::ProjectLoadoutConflict); }
            let policy:Option<Vec<u8>>=tx.query_row("SELECT policy_fingerprint FROM project_policy_publications WHERE project_id=?1",[project],|r|r.get(0)).optional().map_err(map_sqlite_error)?;
            if policy.is_some_and(|policy| !bool::from(policy.as_slice().ct_eq(fingerprint.as_slice()))) { return Err(AccessStoreError::ProjectLoadoutConflict); }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(())
        }).await
    }

    /// Receipt inspection for native journal recovery. This never establishes caller authority.
    pub(crate) async fn tailcat_enrollment_receipt(
        &self,
        idempotency: [u8; 32],
        request: [u8; 32],
        id: String,
        digest: [u8; 32],
    ) -> Result<bool, AccessStoreError> {
        self.with_connection(move |connection| {
            let receipt: Option<(Vec<u8>,String,String,Vec<u8>)> = connection.query_row(
                "SELECT i.request_digest,i.credential_id,i.status,c.credential_digest FROM credential_idempotency i JOIN project_credentials c ON c.credential_id=i.credential_id WHERE i.idempotency_digest=?1 AND i.operation='issue'",
                [idempotency.as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
            ).optional().map_err(map_sqlite_error)?;
            match receipt {
                None => Ok(false),
                Some((stored,credential,status,canonical)) if credential==id && status=="committed"
                    && bool::from(stored.as_slice().ct_eq(request.as_slice())) && bool::from(canonical.as_slice().ct_eq(digest.as_slice())) => Ok(true),
                Some(_) => Err(AccessStoreError::BootstrapConflict),
            }
        }).await
    }

    pub(crate) async fn enroll_tailcat_credential(
        &self,
        input: TailcatEnrollmentInput,
    ) -> Result<MutationOutcome, AccessStoreError> {
        if input.identity.authenticator() != Authenticator::BrowserSession
            || input.expires_at <= input.now
            || input.expires_at.saturating_sub(input.now) > 86400
            || !labby_runtime::gateway_config::is_canonical_project_id(&input.project_id)
            || input.credential_id.is_empty()
            || input.credential_id.len() > 96
            || input.loadout_id != input.route_id
            || !input.loadout_id.starts_with("tailcat-")
            || input.resource.len() > 2048
        {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        use sha2::Digest as _;
        let expected = hex::encode(sha2::Sha256::digest(input.project_id.as_bytes()));
        if input.loadout_id != format!("tailcat-{}", &expected[..16]) {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        let PrincipalLink::External { issuer: _, subject } = input.identity.principal_link() else {
            return Err(AccessStoreError::NotAuthorized);
        };
        let subject = subject.clone();
        let resource = url::Url::parse(&input.resource)
            .map_err(|_| AccessStoreError::InvalidBootstrapInput)?;
        if resource.scheme() != "https"
            || !resource.username().is_empty()
            || resource.password().is_some()
        {
            return Err(AccessStoreError::InvalidBootstrapInput);
        }
        let issuer = resource.origin().ascii_serialization();
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let principal = super::read::resolve_principal(&tx, &input.identity)?;
            let authority: Option<(String, i64, i64, i64, String, i64)> = tx.query_row(
                "SELECT m.role,m.updated_at,o.policy_epoch,p.project_policy_epoch,l.loadout_name,l.updated_at
                 FROM project_memberships m JOIN organizations o ON o.organization_id=m.organization_id
                 JOIN projects p ON p.organization_id=m.organization_id AND p.project_id=m.project_id
                 JOIN project_loadouts l ON l.organization_id=m.organization_id AND l.project_id=m.project_id
                 WHERE m.organization_id=?1 AND m.project_id=?2 AND m.principal_id=?3
                   AND m.status='active' AND o.status='active' AND p.status='active'",
                params![principal.organization_id,input.project_id,principal.id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
            ).optional().map_err(map_sqlite_error)?;
            let Some((role, membership, organization_epoch, project_epoch, assigned, assignment)) = authority else { return Err(AccessStoreError::ProjectLoadoutConflict); };
            if role != "owner" { return Err(AccessStoreError::NotAuthorized); }
            if assigned != input.loadout_id { return Err(AccessStoreError::ProjectLoadoutConflict); }
            if input.installation_id.is_empty() || input.installation_id.len()>160 { return Err(AccessStoreError::InvalidBootstrapInput); }
            tx.execute("INSERT INTO access_installations(singleton,installation_id,installation_generation,created_at,updated_at) VALUES(1,?1,1,?2,?2) ON CONFLICT(singleton) DO NOTHING", params![input.installation_id,input.now]).map_err(map_sqlite_error)?;
            let (installation, installation_generation): (String,i64) = tx.query_row("SELECT installation_id,installation_generation FROM access_installations WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?))).map_err(map_sqlite_error)?;
            if installation != input.installation_id { return Err(AccessStoreError::BootstrapConflict); }
            let conflicting_issuer: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM project_credentials WHERE installation_id=?1 AND canonical_issuer<>?2)", params![installation,issuer], |r| r.get(0)).map_err(map_sqlite_error)?;
            if conflicting_issuer { return Err(AccessStoreError::BootstrapConflict); }
            let publication: Option<(Vec<u8>,i64)> = tx.query_row("SELECT policy_fingerprint,policy_epoch FROM project_policy_publications WHERE project_id=?1", [&input.project_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(map_sqlite_error)?;
            let policy_epoch = match publication {
                Some((fingerprint,epoch)) if bool::from(fingerprint.as_slice().ct_eq(input.policy_fingerprint.as_slice())) => epoch,
                Some(_) => return Err(AccessStoreError::ProjectLoadoutConflict),
                None => {
                    let existing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM project_credentials WHERE project_id=?1 AND status='active' AND expires_at>?2)", params![input.project_id,input.now], |r| r.get(0)).map_err(map_sqlite_error)?;
                    if existing { return Err(AccessStoreError::ProjectLoadoutConflict); }
                    tx.execute("INSERT INTO project_policy_publications(project_id,policy_fingerprint,policy_epoch,updated_at) VALUES(?1,?2,1,?3)", params![input.project_id,input.policy_fingerprint.as_slice(),input.now]).map_err(map_sqlite_error)?;
                    1
                }
            };
            let prior: Option<(Vec<u8>,String,String)> = tx.query_row("SELECT request_digest,credential_id,status FROM credential_idempotency WHERE idempotency_digest=?1 AND operation='issue'", [input.idempotency_digest.as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(map_sqlite_error)?;
            if let Some((digest,id,status)) = prior {
                if !bool::from(digest.as_slice().ct_eq(input.request_digest.as_slice())) || id != input.credential_id || status != "committed" { return Err(AccessStoreError::BootstrapConflict); }
                let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM project_credentials c WHERE c.credential_id=?1 AND c.credential_digest=?2 AND c.status='active' AND c.expires_at>?3 AND c.principal_id=?4 AND c.project_id=?5 AND c.membership_generation=?6 AND c.organization_policy_epoch=?7 AND c.project_policy_epoch=?8 AND c.loadout_assignment_generation=?9 AND c.expires_at=?10 AND c.loadout_policy_fingerprint=?11 AND c.loadout_id=?12 AND c.route_id=?13 AND c.resource=?14 AND c.loadout_generation=?15 AND c.route_generation=?15 AND c.catalog_generation=?15 AND c.canonical_issuer=?16 AND c.subject=?17 AND c.scopes_json='[\"lab\"]' AND c.installation_generation=?18 AND c.installation_id=?19 AND NOT EXISTS(SELECT 1 FROM access_tombstones t WHERE t.artifact_kind='credential' AND (t.public_id=c.credential_id OR t.canonical_digest=c.credential_digest)))", params![id,input.credential_digest.as_slice(),input.now,principal.id,input.project_id,membership,organization_epoch,project_epoch,assignment,input.expires_at,input.policy_fingerprint.as_slice(),input.loadout_id,input.route_id,input.resource,policy_epoch,issuer,subject,installation_generation,installation], |r| r.get(0)).map_err(map_sqlite_error)?;
                if !current { return Err(AccessStoreError::NotAuthorized); }
                tx.commit().map_err(map_sqlite_error)?;
                return Ok(MutationOutcome::AlreadyApplied);
            }
            let tombstoned: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM access_tombstones WHERE artifact_kind='credential' AND (public_id=?1 OR canonical_digest=?2))", params![input.credential_id,input.credential_digest.as_slice()], |r| r.get(0)).map_err(map_sqlite_error)?;
            if tombstoned { return Err(AccessStoreError::NotAuthorized); }
            tx.execute("INSERT INTO project_credentials(credential_id,installation_id,installation_generation,credential_digest,credential_generation,canonical_issuer,subject,organization_id,principal_id,project_id,membership_generation,organization_policy_epoch,project_policy_epoch,loadout_id,loadout_generation,loadout_assignment_generation,catalog_generation,loadout_policy_fingerprint,route_id,route_generation,resource,audience,scopes_json,status,issued_at,expires_at,revoked_at,revocation_generation,updated_at) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?14,?16,?17,?14,?18,?18,'[\"lab\"]','active',?19,?20,NULL,0,?19)", params![input.credential_id,installation,installation_generation,input.credential_digest.as_slice(),issuer,subject,principal.organization_id,principal.id,input.project_id,membership,organization_epoch,project_epoch,input.loadout_id,policy_epoch,assignment,input.policy_fingerprint.as_slice(),input.route_id,input.resource,input.now,input.expires_at]).map_err(map_sqlite_error)?;
            tx.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'local_credential',NULL,NULL,?3,'active',1,1,?4,?4)", params![format!("credential-link:{}",input.credential_id),principal.id,input.credential_id,input.now]).map_err(map_sqlite_error)?;
            tx.execute("INSERT INTO credential_idempotency(idempotency_digest,installation_id,operation,request_digest,proof_id,credential_id,status,created_at,updated_at) VALUES(?1,?2,'issue',?3,NULL,?4,'committed',?5,?5)", params![input.idempotency_digest.as_slice(),installation,input.request_digest.as_slice(),input.credential_id,input.now]).map_err(map_sqlite_error)?;
            tx.execute("INSERT INTO access_security_events(event_id,occurred_at,event_kind,decision,reason_code,target_fingerprint,peer_fingerprint,metadata_json) VALUES(?1,?2,'credential_issue','allow','tailcat_enrollment',?3,NULL,'{}')", params![ulid::Ulid::new().to_string(),input.now,input.credential_digest.as_slice()]).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)?;
            Ok(MutationOutcome::Created)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AssignProjectLoadoutInput, BootstrapOwnerInput};
    use sha2::{Digest as _, Sha256};
    async fn fixture() -> (tempfile::TempDir, AccessStore, TailcatEnrollmentInput) {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "real-fixture-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        let hash = hex::encode(Sha256::digest(b"bootstrap-default"));
        let name = format!("tailcat-{}", &hash[..16]);
        store
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", name.clone())
                    .unwrap(),
            )
            .await
            .unwrap();
        let input = TailcatEnrollmentInput {
            identity,
            installation_id: "native-test-installation".into(),
            project_id: "bootstrap-default".into(),
            loadout_id: name.clone(),
            route_id: name,
            resource: "https://labby.example/sandbox".into(),
            policy_fingerprint: [4; 32],
            credential_id: "credential-one".into(),
            credential_digest: [5; 32],
            request_digest: [6; 32],
            idempotency_digest: [7; 32],
            now: 100,
            expires_at: 100 + 86400,
        };
        (directory, store, input)
    }

    #[tokio::test]
    async fn enable_fence_requires_current_enrolled_credential_and_is_repeatable() {
        let (_directory, store, mut input) = fixture().await;
        input.now = labby_auth::util::now_unix();
        input.expires_at = input.now + 86400;
        let invoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = invoked.clone();
        assert!(
            store
                .commit_tailcat_enrolled_configuration(input.clone(), move || probe
                    .store(true, std::sync::atomic::Ordering::SeqCst))
                .await
                .is_err()
        );
        assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
        store
            .enroll_tailcat_credential(input.clone())
            .await
            .unwrap();
        assert_eq!(
            store
                .commit_tailcat_enrolled_configuration(input.clone(), || "enabled")
                .await
                .unwrap(),
            "enabled"
        );
        assert_eq!(
            store
                .commit_tailcat_enrolled_configuration(input.clone(), || "enabled")
                .await
                .unwrap(),
            "enabled"
        );
        store
            .execute_test_statement("UPDATE project_credentials SET status='revoked',revoked_at=1;")
            .await
            .unwrap();
        let probe = invoked.clone();
        assert!(
            store
                .commit_tailcat_enrolled_configuration(input, move || probe
                    .store(true, std::sync::atomic::Ordering::SeqCst))
                .await
                .is_err()
        );
        assert!(!invoked.load(std::sync::atomic::Ordering::SeqCst));
    }
    async fn counts(store: &AccessStore) -> (i64, i64, i64, i64, i64) {
        store.with_connection(|connection|connection.query_row("SELECT (SELECT count(*) FROM project_credentials),(SELECT count(*) FROM credential_idempotency),(SELECT count(*) FROM project_policy_publications),(SELECT count(*) FROM access_installations),(SELECT policy_epoch FROM organizations WHERE organization_id='bootstrap-local')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(map_sqlite_error)).await.unwrap()
    }
    #[tokio::test]
    async fn enrolled_credential_is_idempotent_without_changing_organization_authority() {
        let (_directory, store, input) = fixture().await;
        let before = counts(&store).await;
        assert_eq!(
            store
                .enroll_tailcat_credential(input.clone())
                .await
                .unwrap(),
            MutationOutcome::Created
        );
        assert_eq!(
            store
                .enroll_tailcat_credential(input.clone())
                .await
                .unwrap(),
            MutationOutcome::AlreadyApplied
        );
        let snapshot = store
            .introspect_project_credential(input.credential_id.clone(), 1, 100)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.canonical_issuer, "https://labby.example");
        assert_eq!(snapshot.subject, "real-fixture-owner");
        assert_eq!(snapshot.scopes_json, "[\"lab\"]");
        let after = counts(&store).await;
        assert_eq!(after, (1, 1, 1, 1, before.4));
        assert!(
            store
                .tailcat_enrollment_receipt(
                    input.idempotency_digest,
                    input.request_digest,
                    input.credential_id,
                    input.credential_digest
                )
                .await
                .unwrap()
        );
    }
    #[tokio::test]
    async fn wrong_assignment_and_non_owner_never_issue_credentials() {
        let (_directory, store, input) = fixture().await;
        store.with_connection(|connection| connection.execute("UPDATE project_loadouts SET loadout_name='tailcat-other' WHERE project_id='bootstrap-default'", []).map(drop).map_err(map_sqlite_error)).await.unwrap();
        assert!(matches!(
            store.enroll_tailcat_credential(input).await,
            Err(AccessStoreError::ProjectLoadoutConflict)
        ));
        assert_eq!(counts(&store).await.0, 0);
        let (_directory, store, input) = fixture().await;
        store.with_connection(|connection|connection.execute("UPDATE project_memberships SET role='admin' WHERE project_id='bootstrap-default'",[]).map(drop).map_err(map_sqlite_error)).await.unwrap();
        assert!(matches!(
            store.enroll_tailcat_credential(input).await,
            Err(AccessStoreError::NotAuthorized)
        ));
        assert_eq!(counts(&store).await.0, 0);
    }
    #[tokio::test]
    async fn failed_link_insert_rolls_back_entire_enrollment() {
        let (_directory, store, input) = fixture().await;
        let before = counts(&store).await;
        store.with_connection(|connection|connection.execute_batch("CREATE TEMP TRIGGER fail_tailcat_link BEFORE INSERT ON principal_links WHEN NEW.link_kind='local_credential' BEGIN SELECT RAISE(ABORT,'fixture-failure'); END;").map_err(map_sqlite_error)).await.unwrap();
        assert!(store.enroll_tailcat_credential(input).await.is_err());
        assert_eq!(counts(&store).await, before);
    }
    #[tokio::test]
    async fn changed_replay_policy_or_operation_is_refused() {
        let (_directory, store, input) = fixture().await;
        store
            .enroll_tailcat_credential(input.clone())
            .await
            .unwrap();
        let mut changed = input.clone();
        changed.policy_fingerprint = [9; 32];
        assert!(store.enroll_tailcat_credential(changed).await.is_err());
        let mut changed = input;
        changed.request_digest = [8; 32];
        assert!(store.enroll_tailcat_credential(changed).await.is_err());
        assert_eq!(counts(&store).await.0, 1);
    }
}
