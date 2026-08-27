//! Optional exact-revision Depot acquisition adapter.

use std::collections::BTreeSet;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;

use labby_runtime::artifacts::provider::{
    ArtifactFetchPolicy, ArtifactSourceCredential, ExactArtifactRequest, ExactArtifactSource,
    GuardedExactArtifactProvider,
};
use labby_runtime::artifacts::{ArtifactAcquisition, ArtifactError};
use url::Url;

pub(super) type DepotFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ArtifactAcquisition, ArtifactError>> + Send + 'a>>;

pub(super) trait DepotExactProvider: Send + Sync {
    fn acquire(&self, artifact_id: String, revision_id: String) -> DepotFuture<'_>;
}

struct RuntimeDepot {
    provider: GuardedExactArtifactProvider,
    source_id: String,
    endpoint: Url,
    credential_origin: Option<Url>,
    pinned_addresses: BTreeSet<IpAddr>,
    source: ExactArtifactSource,
}

impl DepotExactProvider for RuntimeDepot {
    fn acquire(&self, artifact_id: String, revision_id: String) -> DepotFuture<'_> {
        Box::pin(async move {
            self.provider
                .acquire_exact(&ExactArtifactRequest {
                    source: self.source,
                    source_id: self.source_id.clone(),
                    artifact_id,
                    revision_id,
                    endpoint: self.endpoint.clone(),
                    credential_origin: self.credential_origin.clone(),
                    pinned_addresses: self.pinned_addresses.clone(),
                })
                .await
        })
    }
}

/// Server-held Depot authority. Selectors never contain credentials, endpoints, or paths.
#[derive(Clone)]
pub(crate) struct DepotConnection {
    provider: Arc<dyn DepotExactProvider>,
    source_id: String,
    source: ExactArtifactSource,
}

impl DepotConnection {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn configured(
        source: ExactArtifactSource,
        source_id: impl Into<String>,
        endpoint: Url,
        credential: Option<ArtifactSourceCredential>,
        pinned_addresses: BTreeSet<IpAddr>,
        staging_root: impl Into<std::path::PathBuf>,
        policy: ArtifactFetchPolicy,
    ) -> Result<Self, ArtifactError> {
        let source_id = source_id.into();
        let provider = GuardedExactArtifactProvider::configured_http(
            endpoint.clone(),
            pinned_addresses.clone(),
            credential,
            staging_root,
            policy,
        )?;
        // ExactArtifactRequest performs the authoritative URL/DNS/credential validation before IO.
        let credential_origin = Some(endpoint.clone());
        let runtime = RuntimeDepot {
            provider,
            source_id: source_id.clone(),
            endpoint,
            credential_origin,
            pinned_addresses,
            source,
        };
        Ok(Self {
            provider: Arc::new(runtime),
            source_id,
            source,
        })
    }

    #[cfg(test)]
    pub(super) fn fake(
        provider: Arc<dyn DepotExactProvider>,
        source_id: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            source_id: source_id.into(),
            source: ExactArtifactSource::Depot,
        }
    }

    pub(crate) async fn acquire_exact(
        &self,
        artifact_id: String,
        revision_id: String,
    ) -> Result<ArtifactAcquisition, ArtifactError> {
        let acquisition = self
            .provider
            .acquire(artifact_id.clone(), revision_id.clone())
            .await?;
        acquisition.validate()?;
        let provenance_matches = match self.source {
            ExactArtifactSource::Depot => {
                acquisition.interchange.provenance.provider.as_deref() == Some("depot")
                    && acquisition.interchange.provenance.registry.as_deref()
                        == Some(&self.source_id)
            }
            ExactArtifactSource::Repository => {
                acquisition.interchange.provenance.provider.as_deref() == Some("repository")
                    && acquisition.interchange.provenance.repository.as_deref()
                        == Some(&self.source_id)
                    && acquisition.interchange.provenance.reference.as_deref()
                        == Some(revision_id.as_str())
            }
        };
        if acquisition.interchange.descriptor.id != artifact_id
            || acquisition.interchange.revision.id != revision_id
            || !provenance_matches
        {
            return Err(ArtifactError::Conflict("depot_exact_object_mismatch"));
        }
        Ok(acquisition)
    }

    pub(crate) fn connection_id(&self) -> &str {
        &self.source_id
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use labby_runtime::artifacts::{
        ArtifactPayloadFile, ArtifactProvenance, LogicalSkillFile, materialize_logical_skill,
    };

    use super::*;

    struct FakeDepot {
        acquisition: ArtifactAcquisition,
        calls: AtomicUsize,
        denied: bool,
    }

    impl DepotExactProvider for FakeDepot {
        fn acquire(&self, _artifact_id: String, _revision_id: String) -> DepotFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if self.denied {
                    Err(ArtifactError::Conflict("source_authorization_denied"))
                } else {
                    Ok(self.acquisition.clone())
                }
            })
        }
    }

    fn acquisition() -> ArtifactAcquisition {
        let content = "---\nname: depot-demo\ndescription: depot\n---\nbody\n";
        let provenance = ArtifactProvenance {
            provider: Some("depot".to_owned()),
            registry: Some("account-1".to_owned()),
            reference: Some("immutable-object".to_owned()),
            ..ArtifactProvenance::default()
        };
        let materialized = materialize_logical_skill(
            "depot-demo",
            vec![LogicalSkillFile::new("SKILL.md", content)],
            provenance,
        )
        .unwrap();
        ArtifactAcquisition {
            interchange: materialized.interchange,
            files: vec![ArtifactPayloadFile {
                path: "SKILL.md".to_owned(),
                bytes: content.as_bytes().to_vec(),
            }],
        }
    }

    #[tokio::test]
    async fn exact_depot_identity_and_source_authorization_are_fail_closed() {
        let expected = acquisition();
        let artifact_id = expected.interchange.descriptor.id.clone();
        let revision_id = expected.interchange.revision.id.clone();
        let provider = Arc::new(FakeDepot {
            acquisition: expected.clone(),
            calls: AtomicUsize::new(0),
            denied: false,
        });
        let connection = DepotConnection::fake(provider.clone(), "account-1");
        assert_eq!(
            connection
                .acquire_exact(artifact_id.clone(), revision_id.clone())
                .await
                .unwrap(),
            expected
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let mut tampered = acquisition();
        tampered.interchange.provenance.registry = Some("other-account".to_owned());
        let connection = DepotConnection::fake(
            Arc::new(FakeDepot {
                acquisition: tampered,
                calls: AtomicUsize::new(0),
                denied: false,
            }),
            "account-1",
        );
        assert!(matches!(
            connection
                .acquire_exact(artifact_id.clone(), revision_id.clone())
                .await,
            Err(ArtifactError::Conflict("depot_exact_object_mismatch"))
        ));

        let connection = DepotConnection::fake(
            Arc::new(FakeDepot {
                acquisition: acquisition(),
                calls: AtomicUsize::new(0),
                denied: true,
            }),
            "account-1",
        );
        assert!(matches!(
            connection.acquire_exact(artifact_id, revision_id).await,
            Err(ArtifactError::Conflict("source_authorization_denied"))
        ));
    }
}
