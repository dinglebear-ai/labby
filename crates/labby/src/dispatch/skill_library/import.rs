//! Server-to-server Skill acquisition terminating at the canonical import dispatch.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::{collections::BTreeMap, collections::BTreeSet, path::Path};

use labby_runtime::artifacts::{ArtifactAcquisition, ArtifactError};
use serde_json::Value;

use crate::access::AccessRuntime;

use super::audit::SkillLibraryCorrelationId;
use super::auth::SkillLibraryCaller;
use super::depot::DepotConnection;
use super::dispatch::{SkillLibraryDispatchError, SkillLibraryService};
use super::params::SourceSelector;

pub(crate) type RepositoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ArtifactAcquisition, ArtifactError>> + Send + 'a>>;

/// A configured server-side repository connection. Implementations own credentials and auth.
pub(crate) trait RepositoryConnection: Send + Sync {
    fn acquire_exact<'a>(
        &'a self,
        repository: &'a str,
        artifact_id: &'a str,
        object_id: &'a str,
    ) -> RepositoryFuture<'a>;
}

impl RepositoryConnection for DepotConnection {
    fn acquire_exact<'a>(
        &'a self,
        repository: &'a str,
        artifact_id: &'a str,
        object_id: &'a str,
    ) -> RepositoryFuture<'a> {
        Box::pin(async move {
            if self.connection_id() != repository {
                return Err(ArtifactError::NotFound("import_connection"));
            }
            DepotConnection::acquire_exact(self, artifact_id.to_owned(), object_id.to_owned()).await
        })
    }
}

#[derive(Clone)]
pub(crate) enum ImportSource {
    Depot {
        connection_id: String,
        artifact_id: String,
        revision_id: String,
    },
    Repository {
        repository: String,
        artifact_id: String,
        object_id: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ImportAdapterError {
    #[error("requested import source is not configured")]
    SourceUnavailable,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Dispatch(#[from] SkillLibraryDispatchError),
}

/// Optional adapters plus bounded acquisition policy. Absence is local-only mode, not fallback.
pub(crate) struct ImportCoordinator {
    depot: BTreeMap<String, DepotConnection>,
    repository: BTreeMap<String, Arc<dyn RepositoryConnection>>,
}

impl ImportCoordinator {
    pub(crate) fn from_config(
        config: &crate::config::SkillLibraryPreferences,
        staging_root: &Path,
    ) -> Result<Self, ArtifactError> {
        let mut depot = BTreeMap::new();
        let mut repository: BTreeMap<String, Arc<dyn RepositoryConnection>> = BTreeMap::new();
        for source in &config.sources {
            labby_runtime::artifacts::validation::validate_id(&source.id, "connection_id")?;
            let endpoint =
                url::Url::parse(&source.endpoint).map_err(|_| ArtifactError::InvalidField {
                    field: "source.endpoint",
                    reason: "invalid_url",
                })?;
            let credential = source
                .bearer_token_env
                .as_ref()
                .map(|name| {
                    std::env::var(name)
                        .map_err(|_| ArtifactError::NotFound("source_credential"))
                        .and_then(|secret| {
                            labby_runtime::artifacts::provider::ArtifactSourceCredential::bearer(
                                &secret,
                            )
                        })
                })
                .transpose()?;
            let source_root = staging_root.join(&source.id);
            std::fs::create_dir_all(&source_root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&source_root, std::fs::Permissions::from_mode(0o700))?;
            }
            let kind = match source.kind {
                crate::config::SkillLibrarySourceKind::Depot => {
                    labby_runtime::artifacts::provider::ExactArtifactSource::Depot
                }
                crate::config::SkillLibrarySourceKind::Repository => {
                    labby_runtime::artifacts::provider::ExactArtifactSource::Repository
                }
            };
            let connection = DepotConnection::configured(
                kind,
                source.id.clone(),
                endpoint,
                credential,
                source
                    .pinned_addresses
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>(),
                source_root,
                Default::default(),
            )?;
            let duplicate = match source.kind {
                crate::config::SkillLibrarySourceKind::Depot => {
                    depot.insert(source.id.clone(), connection).is_some()
                }
                crate::config::SkillLibrarySourceKind::Repository => repository
                    .insert(source.id.clone(), Arc::new(connection))
                    .is_some(),
            };
            if duplicate {
                return Err(ArtifactError::Conflict("duplicate_import_connection_id"));
            }
        }
        Ok(Self { depot, repository })
    }

    #[cfg(test)]
    pub(crate) fn new(
        depot: Option<DepotConnection>,
        repository: Option<Arc<dyn RepositoryConnection>>,
    ) -> Self {
        let depot = depot
            .into_iter()
            .map(|connection| (connection.connection_id().to_owned(), connection))
            .collect();
        let repository = repository
            .into_iter()
            .map(|connection| ("repo-1".to_owned(), connection))
            .collect();
        Self { depot, repository }
    }

    async fn acquire(
        &self,
        source: ImportSource,
    ) -> Result<ArtifactAcquisition, ImportAdapterError> {
        let acquisition = match source {
            ImportSource::Depot {
                connection_id,
                artifact_id,
                revision_id,
            } => self
                .depot
                .get(&connection_id)
                .ok_or(ImportAdapterError::SourceUnavailable)?
                .acquire_exact(artifact_id, revision_id)
                .await
                .map_err(ImportAdapterError::Artifact),
            ImportSource::Repository {
                repository,
                artifact_id,
                object_id,
            } => {
                validate_exact_repository_selector(&repository, &object_id)?;
                let acquisition = self
                    .repository
                    .get(&repository)
                    .ok_or(ImportAdapterError::SourceUnavailable)?
                    .acquire_exact(&repository, &artifact_id, &object_id)
                    .await
                    .map_err(ImportAdapterError::Artifact)?;
                if acquisition.interchange.provenance.provider.as_deref() != Some("repository")
                    || acquisition.interchange.provenance.repository.as_deref()
                        != Some(repository.as_str())
                    || acquisition.interchange.provenance.reference.as_deref()
                        != Some(object_id.as_str())
                {
                    return Err(ArtifactError::Conflict("repository_exact_object_mismatch").into());
                }
                Ok(acquisition)
            }
        }?;
        acquisition.validate()?;
        Ok(acquisition)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn import<G: Send + Sync + 'static>(
        &self,
        service: &SkillLibraryService<G>,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        source: ImportSource,
        expected_library_version: u64,
        idempotency_key: String,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, ImportAdapterError> {
        let acquisition = self.acquire(source).await?;
        service
            .import_acquired(
                runtime,
                caller,
                project_id,
                acquisition,
                expected_library_version,
                idempotency_key,
                correlation_id,
            )
            .await
            .map_err(ImportAdapterError::Dispatch)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn import_selected<G: Send + Sync + 'static>(
        &self,
        service: &SkillLibraryService<G>,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        source: SourceSelector,
        expected_library_version: u64,
        idempotency_key: String,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, ImportAdapterError> {
        let source = match source {
            SourceSelector::Depot {
                connection_id,
                artifact_id,
                revision_id,
            } => {
                if !self.depot.contains_key(&connection_id) {
                    return Err(ImportAdapterError::SourceUnavailable);
                }
                ImportSource::Depot {
                    connection_id,
                    artifact_id,
                    revision_id,
                }
            }
            SourceSelector::Repository {
                connection_id,
                artifact_id,
                revision_id,
            } => ImportSource::Repository {
                repository: connection_id,
                artifact_id,
                object_id: revision_id,
            },
        };
        self.import(
            service,
            runtime,
            caller,
            project_id,
            source,
            expected_library_version,
            idempotency_key,
            correlation_id,
        )
        .await
    }
}

fn validate_exact_repository_selector(
    repository: &str,
    object_id: &str,
) -> Result<(), ImportAdapterError> {
    let invalid = |value: &str| {
        value.is_empty()
            || value.len() > 512
            || value.chars().any(char::is_control)
            || value.contains('/')
            || value.contains('\\')
            || value.starts_with('-')
    };
    if invalid(repository) || invalid(object_id) || !object_id.starts_with("sha256:") {
        return Err(ArtifactError::InvalidField {
            field: "repository_object",
            reason: "exact_object_required",
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_runtime::artifacts::{
        ArtifactPayloadFile, ArtifactProvenance, LogicalSkillFile, materialize_logical_skill,
    };

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};
    use crate::dispatch::skill_library::auth::SkillLibraryTransport;
    use crate::dispatch::skill_library::blocking::BoundedBlockingExecutor;
    use crate::dispatch::skill_library::depot::{DepotExactProvider, DepotFuture};
    use crate::dispatch::skill_library::dispatch::{
        ActivationCoordinator, ArtifactFirstPartyProjection, GenerationProjection,
    };
    use serde_json::json;

    fn acquisition(
        name: &str,
        provider: &str,
        registry: Option<&str>,
        repository: Option<&str>,
        reference: &str,
    ) -> ArtifactAcquisition {
        let content = format!("---\nname: {name}\ndescription: imported\n---\nbody\n");
        let provenance = ArtifactProvenance {
            provider: Some(provider.to_owned()),
            registry: registry.map(str::to_owned),
            repository: repository.map(str::to_owned),
            reference: Some(reference.to_owned()),
            ..ArtifactProvenance::default()
        };
        let materialized = materialize_logical_skill(
            name,
            vec![LogicalSkillFile::new("SKILL.md", content.clone())],
            provenance,
        )
        .unwrap();
        ArtifactAcquisition {
            interchange: materialized.interchange,
            files: vec![ArtifactPayloadFile {
                path: "SKILL.md".to_owned(),
                bytes: content.into_bytes(),
            }],
        }
    }

    #[test]
    fn owned_acquisition_validation_preserves_payload_allocation() {
        let acquisition = acquisition(
            "zero-copy",
            "depot",
            Some("account-1"),
            None,
            "sha256:exact",
        );
        let pointer = acquisition.files[0].bytes.as_ptr();
        let capacity = acquisition.files[0].bytes.capacity();
        let acquisition = super::super::dispatch::validate_owned_acquisition(acquisition).unwrap();
        assert_eq!(acquisition.files[0].bytes.as_ptr(), pointer);
        assert_eq!(acquisition.files[0].bytes.capacity(), capacity);
    }

    struct FakeDepot {
        value: ArtifactAcquisition,
        calls: Arc<AtomicUsize>,
    }

    impl DepotExactProvider for FakeDepot {
        fn acquire(&self, _artifact_id: String, _revision_id: String) -> DepotFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(self.value.clone()) })
        }
    }

    struct FakeRepository {
        value: Result<ArtifactAcquisition, &'static str>,
        calls: Arc<AtomicUsize>,
    }

    impl RepositoryConnection for FakeRepository {
        fn acquire_exact<'a>(
            &'a self,
            _repository: &'a str,
            _artifact_id: &'a str,
            _object_id: &'a str,
        ) -> RepositoryFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { self.value.clone().map_err(ArtifactError::Conflict) })
        }
    }

    #[tokio::test]
    async fn exact_sources_import_idempotently_then_run_entirely_local() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let access_path = root.path().join("access.db");
        let access_store = AccessStore::open(access_path.clone()).await.unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner-subject",
        )
        .unwrap();
        access_store
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        drop(access_store);
        let runtime = AccessRuntime::initialize(access_path).await;
        let caller = || {
            SkillLibraryCaller::new(
                identity.clone(),
                [],
                SkillLibraryTransport::browser(true, true),
            )
        };

        let store = Arc::new(
            labby_runtime::artifacts::ArtifactStore::new(root.path().join("artifacts")).unwrap(),
        );
        let projection: Arc<
            dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
        > = Arc::new(ArtifactFirstPartyProjection);
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let publication = Arc::new(ActivationCoordinator::new(initial, 0));
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            BoundedBlockingExecutor::new(2, Duration::from_secs(1), Duration::from_secs(10))
                .unwrap(),
            publication,
            projection,
        );

        let depot = acquisition("depot-import", "depot", Some("account-1"), None, "object-1");
        let depot_id = depot.interchange.descriptor.id.clone();
        let depot_revision = depot.interchange.revision.id.clone();
        assert!(matches!(
            service
                .dispatch(
                    &runtime,
                    caller(),
                    "bootstrap-default",
                    "skill_library.import",
                    json!({
                        "acquisition": {
                            "interchange": depot.interchange.clone(),
                            "files": depot.files.iter().map(|file| json!({
                                "path": file.path,
                                "content": String::from_utf8_lossy(&file.bytes)
                            })).collect::<Vec<_>>()
                        },
                        "expected_library_version": 0,
                        "idempotency_key": "raw-public-payload"
                    }),
                    &SkillLibraryCorrelationId::parse("raw-public-import").unwrap(),
                )
                .await,
            Err(SkillLibraryDispatchError::InvalidParams)
        ));
        let depot_calls = Arc::new(AtomicUsize::new(0));
        let repository = acquisition(
            "repo-import",
            "repository",
            None,
            Some("repo-1"),
            "sha256:repository-object",
        );
        let repository_id = repository.interchange.descriptor.id.clone();
        let repository_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = ImportCoordinator::new(
            Some(DepotConnection::fake(
                Arc::new(FakeDepot {
                    value: depot,
                    calls: Arc::clone(&depot_calls),
                }),
                "account-1",
            )),
            Some(Arc::new(FakeRepository {
                value: Ok(repository),
                calls: Arc::clone(&repository_calls),
            })),
        );

        let first = coordinator
            .import_selected(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                SourceSelector::Depot {
                    connection_id: "account-1".to_owned(),
                    artifact_id: depot_id.clone(),
                    revision_id: depot_revision.clone(),
                },
                0,
                "depot-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("depot-import-1").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first["outcome"], "committed");
        let replay = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Depot {
                    connection_id: "account-1".to_owned(),
                    artifact_id: depot_id.clone(),
                    revision_id: depot_revision.clone(),
                },
                0,
                "depot-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("depot-import-1").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay["outcome"], "replayed");
        assert_eq!(store.library_snapshot().unwrap().version, 1);

        let repo_result = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: repository_id,
                    object_id: "sha256:repository-object".to_owned(),
                },
                1,
                "repo-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("repo-import-2").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(repo_result["outcome"], "committed");
        assert_eq!(store.library_snapshot().unwrap().records.len(), 2);
        assert!(matches!(
            coordinator
                .import(
                    &service,
                    &runtime,
                    caller(),
                    "bootstrap-default",
                    ImportSource::Depot {
                        connection_id: "account-1".to_owned(),
                        artifact_id: depot_id.clone(),
                        revision_id: depot_revision.clone(),
                    },
                    2,
                    "collision-key".to_owned(),
                    &SkillLibraryCorrelationId::parse("collision-3").unwrap(),
                )
                .await,
            Err(ImportAdapterError::Dispatch(
                SkillLibraryDispatchError::Artifact(ArtifactError::Conflict("artifact_exists"))
            ))
        ));

        let calls_before_unplug = (
            depot_calls.load(Ordering::SeqCst),
            repository_calls.load(Ordering::SeqCst),
        );
        drop(coordinator);
        service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "skill_library.activate",
                json!({
                    "artifact_id": depot_id.clone(),
                    "expected_revision_id": depot_revision.clone(),
                    "expected_library_version": 2,
                    "idempotency_key": "activate-local"
                }),
                &SkillLibraryCorrelationId::parse("activate-local-4").unwrap(),
            )
            .await
            .unwrap();
        let list = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "skill_library.list",
                json!({}),
                &SkillLibraryCorrelationId::parse("list-local-5").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 2);
        let get = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "skill_library.get",
                json!({"artifact_id": depot_id}),
                &SkillLibraryCorrelationId::parse("get-local-6").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get["name"], "depot-import");
        let read = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "skill_library.read",
                json!({
                    "artifact_id": depot_id,
                    "revision_id": depot_revision,
                    "path": "SKILL.md"
                }),
                &SkillLibraryCorrelationId::parse("read-local-7").unwrap(),
            )
            .await
            .unwrap();
        assert!(read["text"].as_str().unwrap().contains("depot-import"));
        assert_eq!(
            calls_before_unplug,
            (
                depot_calls.load(Ordering::SeqCst),
                repository_calls.load(Ordering::SeqCst)
            )
        );
    }

    #[tokio::test]
    async fn repository_product_boundary_preserves_typed_source_failures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = ImportCoordinator::new(
            None,
            Some(Arc::new(FakeRepository {
                value: Err("source_authorization_expired"),
                calls: Arc::clone(&calls),
            })),
        );
        assert!(matches!(
            coordinator
                .acquire(ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: "source-artifact".to_owned(),
                    object_id: "sha256:exact".to_owned(),
                })
                .await,
            Err(ImportAdapterError::Artifact(ArtifactError::Conflict(
                "source_authorization_expired"
            )))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            coordinator
                .acquire(ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: "source-artifact".to_owned(),
                    object_id: "main".to_owned(),
                })
                .await,
            Err(ImportAdapterError::Artifact(
                ArtifactError::InvalidField { .. }
            ))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let timeout = ImportCoordinator::new(
            None,
            Some(Arc::new(FakeRepository {
                value: Err("provider_timeout"),
                calls: Arc::new(AtomicUsize::new(0)),
            })),
        );
        assert!(matches!(
            timeout
                .acquire(ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: "source-artifact".to_owned(),
                    object_id: "sha256:exact".to_owned(),
                })
                .await,
            Err(ImportAdapterError::Artifact(ArtifactError::Conflict(
                "provider_timeout"
            )))
        ));

        let mut partial = acquisition(
            "partial",
            "repository",
            None,
            Some("repo-1"),
            "sha256:exact",
        );
        partial.files.clear();
        let partial = ImportCoordinator::new(
            None,
            Some(Arc::new(FakeRepository {
                value: Ok(partial),
                calls: Arc::new(AtomicUsize::new(0)),
            })),
        );
        assert!(matches!(
            partial
                .acquire(ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: "source-artifact".to_owned(),
                    object_id: "sha256:exact".to_owned(),
                })
                .await,
            Err(ImportAdapterError::Artifact(
                ArtifactError::InvalidField { .. }
            ))
        ));

        let mut tampered = acquisition(
            "tampered",
            "repository",
            None,
            Some("repo-1"),
            "sha256:exact",
        );
        tampered.files[0].bytes.push(b'!');
        let tampered = ImportCoordinator::new(
            None,
            Some(Arc::new(FakeRepository {
                value: Ok(tampered),
                calls: Arc::new(AtomicUsize::new(0)),
            })),
        );
        assert!(matches!(
            tampered
                .acquire(ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: "source-artifact".to_owned(),
                    object_id: "sha256:exact".to_owned(),
                })
                .await,
            Err(ImportAdapterError::Artifact(ArtifactError::Conflict(
                "provider_file_size_mismatch" | "provider_file_digest_mismatch"
            )))
        ));
    }

    #[tokio::test]
    async fn local_only_config_has_a_typed_non_fallback_source_failure() {
        let root = tempfile::tempdir().unwrap();
        let coordinator = ImportCoordinator::from_config(
            &crate::config::SkillLibraryPreferences::default(),
            root.path(),
        )
        .unwrap();
        assert!(matches!(
            coordinator
                .acquire(ImportSource::Depot {
                    connection_id: "missing".to_owned(),
                    artifact_id: "artifact".to_owned(),
                    revision_id: format!("sha256:{}", "0".repeat(64)),
                })
                .await,
            Err(ImportAdapterError::SourceUnavailable)
        ));
    }

    #[test]
    fn resolved_config_installs_both_guarded_source_families_without_io() {
        use std::net::{IpAddr, Ipv4Addr};

        drop(rustls::crypto::ring::default_provider().install_default());
        let root = tempfile::tempdir().unwrap();
        let sources = [
            (
                "depot-primary",
                crate::config::SkillLibrarySourceKind::Depot,
            ),
            (
                "repository-primary",
                crate::config::SkillLibrarySourceKind::Repository,
            ),
        ]
        .into_iter()
        .map(|(id, kind)| crate::config::SkillLibrarySourceConfig {
            id: id.to_owned(),
            kind,
            endpoint: format!("https://{id}.example/v1/exact"),
            pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
            bearer_token_env: None,
        })
        .collect();
        let coordinator = ImportCoordinator::from_config(
            &crate::config::SkillLibraryPreferences { sources },
            root.path(),
        )
        .unwrap();
        assert!(coordinator.depot.contains_key("depot-primary"));
        assert!(coordinator.repository.contains_key("repository-primary"));
    }

    #[test]
    fn documented_depot_toml_parses_and_fails_closed_on_credentials_and_duplicates() {
        const TOKEN_ENV: &str = "LABBY_TEST_DEPOT_SOURCE_TOKEN_531";
        drop(rustls::crypto::ring::default_provider().install_default());
        assert!(std::env::var_os(TOKEN_ENV).is_none());

        let config: crate::config::LabConfig = toml::from_str(&format!(
            r#"
[[skill_library.sources]]
id = "unraid-team-depot"
kind = "depot"
endpoint = "https://depot.example.invalid/artifacts/acquire"
pinned_addresses = ["1.1.1.1", "2606:4700:4700::1111"]
bearer_token_env = "{TOKEN_ENV}"
"#
        ))
        .expect("documented Depot source parses");
        assert_eq!(config.skill_library.sources.len(), 1);
        assert_eq!(
            config.skill_library.sources[0].kind,
            crate::config::SkillLibrarySourceKind::Depot
        );
        assert_eq!(config.skill_library.sources[0].pinned_addresses.len(), 2);

        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            ImportCoordinator::from_config(&config.skill_library, root.path()),
            Err(ArtifactError::NotFound("source_credential"))
        ));

        let mut credential_free = config.skill_library.clone();
        credential_free.sources[0].bearer_token_env = None;
        let coordinator = ImportCoordinator::from_config(&credential_free, root.path()).unwrap();
        assert!(coordinator.depot.contains_key("unraid-team-depot"));

        let mut duplicate = credential_free;
        duplicate.sources.push(duplicate.sources[0].clone());
        assert!(matches!(
            ImportCoordinator::from_config(&duplicate, root.path()),
            Err(ArtifactError::Conflict("duplicate_import_connection_id"))
        ));
    }
}
