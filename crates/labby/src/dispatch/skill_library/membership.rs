//! Current-head Depot membership, never a historical revision membership claim.

use labby_runtime::artifacts::{ArtifactError, ArtifactStore, LibrarySnapshot, validation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::auth::SkillLibraryAuthorizationDecision;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selector {
    connection_id: String,
    artifact_id: String,
    revision_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Params {
    items: Vec<Selector>,
}

impl Params {
    pub(crate) fn validate(&self) -> Result<(), ArtifactError> {
        if self.items.is_empty() || self.items.len() > 100 {
            return Err(ArtifactError::InvalidField {
                field: "items",
                reason: "invalid_batch_size",
            });
        }
        for item in &self.items {
            if item.connection_id.is_empty()
                || item.connection_id.len() > 128
                || !item
                    .connection_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err(ArtifactError::InvalidField {
                    field: "connection_id",
                    reason: "invalid_connection_id",
                });
            }
            validation::validate_id(&item.artifact_id, "artifact_id")?;
            validation::validate_reference_id(&item.revision_id, "revision_id")?;
        }
        Ok(())
    }
}

pub(crate) fn lookup(
    store: &ArtifactStore,
    snapshot: &LibrarySnapshot,
    decision: &SkillLibraryAuthorizationDecision,
    params: Params,
) -> Result<Value, ArtifactError> {
    let mut items = Vec::with_capacity(params.items.len());
    for item in params.items {
        let mut status = "absent";
        if let Some(record) = snapshot.records.get(&item.artifact_id).filter(|record| {
            !record.archived
                && decision.permits_record(
                    &record.ownership,
                    record.visibility,
                    record.active_revision_id.is_some(),
                )
        }) {
            let stored = store.get(&item.artifact_id)?;
            if stored.current_revision_id != record.latest_revision_id {
                return Err(ArtifactError::Conflict("library_version_changed"));
            }
            if matches_source(&stored.provenance, &item.connection_id) {
                store.revision(&item.artifact_id, &record.latest_revision_id)?;
                status = if record.latest_revision_id == item.revision_id {
                    "exact_revision_present"
                } else {
                    "different_revision_present"
                };
            }
        }
        items.push(json!({"connection_id":item.connection_id,"artifact_id":item.artifact_id,"revision_id":item.revision_id,"status":status}));
    }
    if store.library_snapshot()?.version != snapshot.version {
        return Err(ArtifactError::Conflict("library_version_changed"));
    }
    Ok(json!({"library_version":snapshot.version,"items":items}))
}

fn matches_source(
    provenance: &labby_runtime::artifacts::ArtifactProvenance,
    connection: &str,
) -> bool {
    provenance.provider.as_deref() == Some("depot")
        && provenance.registry.as_deref() == Some(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_unknown_or_partial_provenance_never_proves_membership() {
        let mut provenance = labby_runtime::artifacts::ArtifactProvenance::default();
        assert!(!matches_source(&provenance, "depot-main"));
        provenance.provider = Some("depot".into());
        assert!(!matches_source(&provenance, "depot-main"));
        provenance.registry = Some("depot-main".into());
        assert!(matches_source(&provenance, "depot-main"));
        assert!(!matches_source(&provenance, "other"));
    }

    #[test]
    fn membership_selectors_are_bounded_and_strict() {
        let item = json!({"connection_id":"depot-main","artifact_id":"skill-one","revision_id":"revision-one"});
        let valid: Params =
            serde_json::from_value(json!({"items":vec![item.clone();100]})).unwrap();
        valid.validate().unwrap();
        for items in [vec![], vec![item.clone(); 101]] {
            assert!(
                serde_json::from_value::<Params>(json!({"items":items}))
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for (field, value) in [
            ("connection_id", "../other"),
            ("artifact_id", "../private"),
            ("revision_id", "../revision"),
        ] {
            let mut invalid = item.clone();
            invalid[field] = value.into();
            assert!(
                serde_json::from_value::<Params>(json!({"items":[invalid]}))
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(
            serde_json::from_value::<Params>(json!({"items":[item],"tenant_id":"other"})).is_err()
        );
    }
}
