//! Optional exact-revision Depot acquisition adapter.

use std::collections::BTreeSet;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;

use labby_runtime::artifacts::provider::{
    ArtifactFetchPolicy, ArtifactRequestHeaderProvider, ArtifactSourceCredential,
    ExactArtifactRequest, ExactArtifactSource, GuardedExactArtifactProvider,
};
use labby_runtime::artifacts::{ArtifactAcquisition, ArtifactError};
use url::Url;

pub(super) type DepotFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ArtifactAcquisition, ArtifactError>> + Send + 'a>>;

/// Per-request delegated headers for one acquisition. `None` means the
/// connection's static credential alone authenticates the request
/// (standalone Depot or repository sources).
pub(crate) type RequestHeaders = Option<Arc<dyn ArtifactRequestHeaderProvider>>;

pub(super) trait DepotExactProvider: Send + Sync {
    fn acquire(
        &self,
        artifact_id: String,
        revision_id: String,
        headers: RequestHeaders,
    ) -> DepotFuture<'_>;
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
    fn acquire(
        &self,
        artifact_id: String,
        revision_id: String,
        headers: RequestHeaders,
    ) -> DepotFuture<'_> {
        Box::pin(async move {
            self.provider
                .acquire_exact_with_headers(
                    &ExactArtifactRequest {
                        source: self.source,
                        source_id: self.source_id.clone(),
                        artifact_id,
                        revision_id,
                        endpoint: self.endpoint.clone(),
                        credential_origin: self.credential_origin.clone(),
                        pinned_addresses: self.pinned_addresses.clone(),
                    },
                    headers.as_deref(),
                )
                .await
        })
    }
}

/// Server-held Depot authority. Selectors never contain credentials, endpoints, or paths.
#[derive(Clone)]
pub(crate) struct DepotConnection {
    catalog_binding: Option<Arc<crate::dispatch::depot::catalog_binding::CatalogBinding>>,
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
            catalog_binding: None,
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
            catalog_binding: None,
            provider,
            source_id: source_id.into(),
            source: ExactArtifactSource::Depot,
        }
    }

    pub(crate) fn bind_catalog(
        &mut self,
        binding: &crate::config::depot::PublicReadBinding,
        acquisition_endpoint: &str,
        token: &str,
    ) -> Result<(), ArtifactError> {
        self.catalog_binding = Some(Arc::new(
            crate::dispatch::depot::catalog_binding::CatalogBinding::new(
                binding,
                acquisition_endpoint,
                token,
            )
            .map_err(ArtifactError::Conflict)?,
        ));
        Ok(())
    }

    pub(crate) async fn acquire_exact(
        &self,
        artifact_id: String,
        revision_id: String,
        headers: RequestHeaders,
    ) -> Result<ArtifactAcquisition, ArtifactError> {
        let authority = match &self.catalog_binding {
            Some(binding) => Some(binding.verify().await.map_err(ArtifactError::Conflict)?),
            None => None,
        };
        let acquisition = self
            .provider
            .acquire(artifact_id.clone(), revision_id.clone(), headers)
            .await?;
        if let (Some(binding), Some(expected)) = (&self.catalog_binding, authority) {
            let current = binding.verify().await.map_err(ArtifactError::Conflict)?;
            if !expected.same_authority(&current) {
                return Err(ArtifactError::Conflict("catalog_authority_changed"));
            }
        }
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
        fn acquire(
            &self,
            _artifact_id: String,
            _revision_id: String,
            _headers: RequestHeaders,
        ) -> DepotFuture<'_> {
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
    async fn public_exact_acquisition_checks_authority_before_and_after_fetch() {
        use crate::dispatch::depot::catalog_binding::CatalogBinding;
        use serde_json::json;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
        struct ChangingDepot {
            value: ArtifactAcquisition,
            authority: Arc<MockServer>,
        }
        impl DepotExactProvider for ChangingDepot {
            fn acquire(&self, _: String, _: String, _: RequestHeaders) -> DepotFuture<'_> {
                Box::pin(async move {
                    self.authority.reset().await;
                    Mock::given(any())
                        .respond_with(ResponseTemplate::new(403))
                        .mount(&self.authority)
                        .await;
                    Ok(self.value.clone())
                })
            }
        }
        let identity = json!({"contractVersion":"depot.discovery/v1","deploymentId":"catalog","deploymentEpoch":"boot","authorityEpoch":"read","listingEpoch":"1","snapshotContinuations":true,"maxPageSize":200});
        for mode in 0..3 {
            let internal = MockServer::start().await;
            let external = Arc::new(MockServer::start().await);
            Mock::given(any())
                .respond_with(ResponseTemplate::new(200).set_body_json(identity.clone()))
                .mount(&internal)
                .await;
            let mut other = identity.clone();
            if mode == 1 {
                other["deploymentId"] = json!("wrong-catalog");
            }
            Mock::given(any())
                .respond_with(ResponseTemplate::new(200).set_body_json(other))
                .mount(&external)
                .await;
            let mut expected = acquisition();
            expected.interchange.provenance.registry = Some("public".into());
            let fake = Arc::new(FakeDepot {
                acquisition: expected.clone(),
                calls: AtomicUsize::new(0),
                denied: false,
            });
            let provider: Arc<dyn DepotExactProvider> = if mode == 2 {
                Arc::new(ChangingDepot {
                    value: expected.clone(),
                    authority: external.clone(),
                })
            } else {
                fake.clone()
            };
            let mut connection = DepotConnection::fake(provider, "public");
            connection.catalog_binding = Some(Arc::new(CatalogBinding::test_origins(
                &internal.uri(),
                &external.uri(),
            )));
            let result = connection
                .acquire_exact(
                    expected.interchange.descriptor.id.clone(),
                    expected.interchange.revision.id.clone(),
                    None,
                )
                .await;
            assert_eq!(result.is_ok(), mode == 0);
            if mode == 0 {
                assert_eq!(result.unwrap(), expected);
            }
            if mode == 1 {
                assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
            }
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
                .acquire_exact(artifact_id.clone(), revision_id.clone(), None)
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
                .acquire_exact(artifact_id.clone(), revision_id.clone(), None)
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
            connection
                .acquire_exact(artifact_id, revision_id, None)
                .await,
            Err(ArtifactError::Conflict("source_authorization_denied"))
        ));
    }
}
