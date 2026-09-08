//! Offline, installation-locked operator consent. This is not an HTTP action.
use crate::access::{AccessStore, owner_link::OwnerLinkApproval};
use crate::installation::{InstallationLifecycleLock, InstallationPaths};
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) async fn prepare(approval_file: PathBuf) -> anyhow::Result<Value> {
    let paths = InstallationPaths::resolve()?;
    let _lease = InstallationLifecycleLock::acquire_offline(&paths)?;
    let bytes = super::secure_file::read_private(&approval_file)?;
    let approval: OwnerLinkApproval = serde_json::from_slice(&bytes)?;
    // Existing identity must exist; preparing consent never bootstraps an installation.
    let installation = super::access_bootstrap::existing_installation_id(&paths)?;
    anyhow::ensure!(
        approval.installation_id == installation,
        "approval installation mismatch"
    );
    let result =
        json!({"prepared":true,"approvalId":approval.approval_id,"expiresAt":approval.expires_at});
    AccessStore::open_existing_current(paths.access_db())
        .await?
        .prepare_owner_link(approval)
        .await?;
    Ok(result)
}
