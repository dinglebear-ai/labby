//! Explicit, offline operator consent for adding an identity to an existing owner.
//! Approval and consumption are immutable audit events; consumption and linking
//! share one immediate transaction, including the unique consumption event ID.
use labby_auth::{PrincipalLink, VerifiedIdentity};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::{AccessStoreError, AccessStoreResult};
use super::store::{AccessStore, map_sqlite_error};

/// Full-width binding for authorization; the short log fingerprint is not a capability.
pub(crate) fn identity_fingerprint(identity: &VerifiedIdentity) -> AccessStoreResult<String> {
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
    Ok(hex::encode(Sha256::digest(
        format!("labby.owner-link.identity.v1\0{issuer}\0{subject}").as_bytes(),
    )))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerLinkApproval {
    pub approval_id: String,
    pub identity_fingerprint: String,
    pub installation_id: String,
    pub principal_id: String,
    pub organization_id: String,
    pub project_id: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub expires_at: i64,
}

impl OwnerLinkApproval {
    fn validate(&self, now: i64) -> AccessStoreResult<()> {
        if self.identity_fingerprint.len() != 64
            || !self
                .identity_fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(AccessStoreError::NotAuthorized);
        }
        if self.expires_at <= now
            || self.expires_at > now.saturating_add(600)
            || [
                &self.approval_id,
                &self.identity_fingerprint,
                &self.installation_id,
                &self.principal_id,
                &self.organization_id,
                &self.project_id,
                &self.loadout_id,
                &self.route_id,
                &self.resource,
            ]
            .iter()
            .any(|value| {
                value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control)
            })
        {
            return Err(AccessStoreError::NotAuthorized);
        }
        Ok(())
    }
}

fn active_target(c: &rusqlite::Connection, a: &OwnerLinkApproval) -> AccessStoreResult<i64> {
    c.query_row(
        "SELECT o.policy_epoch FROM principals u
         JOIN organizations o ON o.organization_id=u.organization_id
         JOIN project_memberships m ON m.principal_id=u.principal_id AND m.organization_id=u.organization_id
         JOIN projects p ON p.project_id=m.project_id AND p.organization_id=m.organization_id
         JOIN project_loadouts l ON l.project_id=p.project_id AND l.organization_id=p.organization_id
         WHERE u.principal_id=?1 AND u.organization_id=?2 AND p.project_id=?3 AND l.loadout_name=?4
         AND u.kind='user' AND u.status='active' AND o.status='active'
         AND m.role='owner' AND m.status='active' AND p.status='active'",
        params![a.principal_id,a.organization_id,a.project_id,a.loadout_id], |row| row.get(0),
    ).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)
}

fn audit(
    c: &rusqlite::Connection,
    a: &OwnerLinkApproval,
    consume: bool,
    epoch: i64,
) -> AccessStoreResult<()> {
    let (prefix, action) = if consume {
        ("owner-link-consume", "access.owner_link.consume")
    } else {
        ("owner-link-prepare", "access.owner_link.prepare")
    };
    let metadata = serde_json::to_string(a).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    c.execute("INSERT INTO access_audit(event_id,occurred_at,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
        VALUES(?1,unixepoch(),?2,?3,?4,?5,'principal_link',?6,'allow','explicit_operator_approval',?7,?8)",
        params![format!("{prefix}:{}",a.approval_id),a.principal_id,a.organization_id,a.project_id,action,a.identity_fingerprint,epoch,metadata])
        .map_err(map_sqlite_error)?;
    Ok(())
}

impl AccessStore {
    pub(crate) async fn owner_link_consumed(
        &self,
        approval: OwnerLinkApproval,
    ) -> AccessStoreResult<bool> {
        self.with_connection(move |c| c.query_row("SELECT EXISTS(SELECT 1 FROM access_audit WHERE event_id=?1 AND action='access.owner_link.consume')", [format!("owner-link-consume:{}",approval.approval_id)],|r|r.get(0)).map_err(map_sqlite_error)).await
    }
    /// Offline operator-only entrypoint. Never expose this method via HTTP/MCP.
    pub(crate) async fn prepare_owner_link(
        &self,
        approval: OwnerLinkApproval,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let now = tx
                .query_row("SELECT unixepoch()", [], |r| r.get(0))
                .map_err(map_sqlite_error)?;
            approval.validate(now)?;
            let epoch = active_target(&tx, &approval)?;
            audit(&tx, &approval, false, epoch)?;
            tx.commit().map_err(map_sqlite_error)
        })
        .await
    }

    /// The caller supplies only middleware-verified identity and current installation.
    /// The browser cannot select another principal, project, or route.
    pub(crate) async fn owner_link_approval(
        &self,
        identity: VerifiedIdentity,
        installation: String,
    ) -> AccessStoreResult<OwnerLinkApproval> {
        self.with_connection(move |c| {
            let metadata:String=c.query_row("SELECT metadata_json FROM access_audit WHERE action='access.owner_link.prepare' AND decision='allow' AND target_fingerprint=?1 ORDER BY occurred_at DESC,event_id DESC LIMIT 1",
                [identity_fingerprint(&identity)?],|r|r.get(0)).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)?;
            let approval:OwnerLinkApproval=serde_json::from_str(&metadata).map_err(|_|AccessStoreError::MalformedVocabulary)?;
            if approval.installation_id!=installation {return Err(AccessStoreError::NotAuthorized);}
            active_target(c,&approval)?;
            Ok(approval)
        }).await
    }

    pub(crate) async fn consume_owner_link(
        &self,
        identity: VerifiedIdentity,
        approval: OwnerLinkApproval,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |c| {
            let PrincipalLink::External{issuer,subject}=identity.principal_link() else {return Err(AccessStoreError::NotAuthorized)};
            if identity_fingerprint(&identity)?!=approval.identity_fingerprint {return Err(AccessStoreError::NotAuthorized);}
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let stored:String=tx.query_row("SELECT metadata_json FROM access_audit WHERE event_id=?1 AND action='access.owner_link.prepare' AND decision='allow'",
                [format!("owner-link-prepare:{}",approval.approval_id)],|r|r.get(0)).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)?;
            let stored:OwnerLinkApproval=serde_json::from_str(&stored).map_err(|_|AccessStoreError::MalformedVocabulary)?;
            if stored!=approval {return Err(AccessStoreError::NotAuthorized);}
            let epoch=active_target(&tx,&approval)?;
            let existing:Option<(String,String)>=tx.query_row("SELECT principal_id,status FROM principal_links WHERE link_kind='external' AND issuer=?1 AND subject=?2",
                params![issuer,subject],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(map_sqlite_error)?;
            if existing.as_ref().is_some_and(|(principal,status)|principal!=&approval.principal_id || status!="active") {return Err(AccessStoreError::NotAuthorized);}
            let consumed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM access_audit WHERE event_id=?1 AND action='access.owner_link.consume')",
                [format!("owner-link-consume:{}",approval.approval_id)],|r|r.get(0)).map_err(map_sqlite_error)?;
            if consumed {
                if existing.is_none() {return Err(AccessStoreError::NotAuthorized);}
                return tx.commit().map_err(map_sqlite_error);
            }
            let now:i64=tx.query_row("SELECT unixepoch()",[],|r|r.get(0)).map_err(map_sqlite_error)?;
            if approval.expires_at<=now {return Err(AccessStoreError::NotAuthorized);}
            if existing.is_none() {
                tx.execute("INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external',?3,?4,NULL,'active',1,1,unixepoch(),unixepoch())",
                    params![format!("owner-link:{}",approval.approval_id),approval.principal_id,issuer,subject]).map_err(map_sqlite_error)?;
            }
            audit(&tx,&approval,true,epoch)?;
            tx.execute("UPDATE access_metadata SET global_revision=global_revision+1,updated_at=unixepoch() WHERE singleton=1",[]).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::BootstrapOwnerInput;
    use labby_auth::Authenticator;

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessStore, OwnerLinkApproval) {
        let temp = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(temp.path().join("access.db"))
            .await
            .unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity("local-owner"), "Team", "Team").unwrap(),
            )
            .await
            .unwrap();
        store.execute_test_statement("INSERT INTO project_loadouts VALUES('bootstrap-local','bootstrap-default','team-skills','bootstrap-owner',1,1)").await.unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let a = OwnerLinkApproval {
            approval_id: "consent-one".into(),
            identity_fingerprint: identity_fingerprint(&identity("google-owner")).unwrap(),
            installation_id: "installation".into(),
            principal_id: "bootstrap-owner".into(),
            organization_id: "bootstrap-local".into(),
            project_id: "bootstrap-default".into(),
            loadout_id: "team-skills".into(),
            route_id: "team-depot-publish".into(),
            resource: "https://example.test/mcp/team-depot".into(),
            expires_at: now + 600,
        };
        (temp, store, a)
    }

    #[tokio::test]
    async fn approval_consumes_once_and_preserves_original_owner() {
        let (temp, store, a) = fixture().await;
        async fn preserved(store: &AccessStore) -> (i64, String) {
            store.with_connection(|c|c.query_row("SELECT (SELECT updated_at FROM project_memberships WHERE principal_id='bootstrap-owner'),json_object('id',link_id,'principal',principal_id,'kind',link_kind,'issuer',issuer,'subject',subject,'credential',credential_id,'status',status,'verification',verification_generation,'generation',link_generation,'created',created_at,'updated',updated_at) FROM principal_links WHERE link_id='bootstrap-owner-link'",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(map_sqlite_error)).await.unwrap()
        }
        let immutable_before = preserved(&store).await;
        let before = store.bootstrap_counts_for_test().await.unwrap();
        store.prepare_owner_link(a.clone()).await.unwrap();
        let (left, right) = tokio::join!(
            store.consume_owner_link(identity("google-owner"), a.clone()),
            store.consume_owner_link(identity("google-owner"), a.clone())
        );
        left.unwrap();
        right.unwrap();
        let after = store.bootstrap_counts_for_test().await.unwrap();
        assert_eq!(after.2, before.2 + 1);
        assert_eq!(after.4, before.4);
        assert_eq!(after.5, before.5 + 2);
        assert_eq!(preserved(&store).await, immutable_before);
        // Canonical integrity includes the original owner's immutable link.
        AccessStore::open_existing_current(temp.path().join("access.db"))
            .await
            .unwrap();
        let selected = store
            .owner_link_approval(identity("google-owner"), "installation".into())
            .await
            .unwrap();
        assert_eq!(selected, a);
    }

    #[tokio::test]
    async fn rejects_unapproved_identity_changed_target_and_revoked_link() {
        let (_temp, store, a) = fixture().await;
        assert!(
            store
                .consume_owner_link(identity("google-owner"), a.clone())
                .await
                .is_err()
        );
        store.prepare_owner_link(a.clone()).await.unwrap();
        assert!(
            store
                .consume_owner_link(identity("other"), a.clone())
                .await
                .is_err()
        );
        let mut changed = a.clone();
        changed.route_id = "other-route".into();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), changed)
                .await
                .is_err()
        );
        store
            .consume_owner_link(identity("google-owner"), a.clone())
            .await
            .unwrap();
        store.execute_test_statement("UPDATE principal_links SET status='revoked' WHERE link_id='owner-link:consent-one'").await.unwrap();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), a)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn expired_approval_and_audit_failure_do_not_link() {
        let (_temp, store, mut a) = fixture().await;
        a.expires_at = 1;
        assert!(store.prepare_owner_link(a).await.is_err());
        let (_temp, store, a) = fixture().await;
        store.prepare_owner_link(a.clone()).await.unwrap();
        store.execute_test_statement("CREATE TEMP TRIGGER owner_link_audit_fail BEFORE INSERT ON access_audit WHEN NEW.action='access.owner_link.consume' BEGIN SELECT RAISE(ABORT,'test'); END").await.unwrap();
        let before = store.bootstrap_counts_for_test().await.unwrap();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), a)
                .await
                .is_err()
        );
        assert_eq!(store.bootstrap_counts_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn expiry_revoked_membership_and_wrong_installation_fail_closed() {
        let (_temp, store, a) = fixture().await;
        store.prepare_owner_link(a.clone()).await.unwrap();
        assert!(
            store
                .owner_link_approval(identity("google-owner"), "other-installation".into())
                .await
                .is_err()
        );
        let mut expired = a.clone();
        expired.expires_at = 1;
        let metadata = serde_json::to_string(&expired).unwrap();
        store.with_connection(move|c|{c.execute("UPDATE access_audit SET metadata_json=?1 WHERE action='access.owner_link.prepare'",[metadata]).map_err(map_sqlite_error)?;Ok(())}).await.unwrap();
        let before = store.bootstrap_counts_for_test().await.unwrap();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), expired)
                .await
                .is_err()
        );
        assert_eq!(store.bootstrap_counts_for_test().await.unwrap(), before);
        let (_temp, store, a) = fixture().await;
        store.prepare_owner_link(a.clone()).await.unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='member' WHERE principal_id='bootstrap-owner'",
            )
            .await
            .unwrap();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), a)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cannot_take_over_an_identity_linked_to_another_principal() {
        let (_temp, store, a) = fixture().await;
        store.prepare_owner_link(a.clone()).await.unwrap();
        store.execute_test_statement("INSERT INTO principals VALUES('other','bootstrap-local','user','active',NULL,1,1); INSERT INTO principal_links VALUES('other-link','other','external','https://accounts.google.com','google-owner',NULL,'active',1,1,1,1)").await.unwrap();
        let before = store.bootstrap_counts_for_test().await.unwrap();
        assert!(
            store
                .consume_owner_link(identity("google-owner"), a)
                .await
                .is_err()
        );
        assert_eq!(store.bootstrap_counts_for_test().await.unwrap(), before);
    }
}
