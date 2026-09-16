//! Server-to-server Skill acquisition terminating at the canonical import dispatch.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::{collections::BTreeMap, collections::BTreeSet, path::Path};

use labby_runtime::artifacts::{ArtifactAcquisition, ArtifactError};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::access::AccessRuntime;

use super::audit::{CanonicalArtifactId, SkillLibraryCorrelationId};
use super::auth::{
    SkillLibraryAction, SkillLibraryCaller, SkillLibraryTarget, authorize_at_boundary,
};
use super::depot::{DepotConnection, RequestHeaders};
use super::dispatch::{SkillLibraryDispatchError, SkillLibraryService};
use super::params::SourceSelector;

const IMPORT_GATE_QUEUE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const IMPORT_GATE_STRIPES: usize = 16;

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
            DepotConnection::acquire_exact(self, artifact_id.to_owned(), object_id.to_owned(), None)
                .await
        })
    }
}

/// Managed Depot exact-artifact routes require a signed delegated read
/// assertion per request. Repository sources and standalone Depot use the
/// connection credential alone. Authorization for the acquisition is the
/// caller's exact `AssetUse` decision on the target Project; the same
/// decision is re-evaluated by `import_acquired` before anything is written.
async fn delegated_read_headers(
    runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    source: &ImportSource,
) -> Result<RequestHeaders, ImportAdapterError> {
    if !matches!(source, ImportSource::Depot { .. }) {
        return Ok(None);
    }
    let Some(controls) = crate::dispatch::skill_library::process_controls() else {
        return Ok(None);
    };
    if !controls.delegation_configured() {
        return Ok(None);
    }
    let context = crate::dispatch::artifact_control::authorize_authority_context(
        runtime,
        caller.identity().clone(),
        project_id,
        caller.selected_team_id(),
        crate::access::Permission::AssetUse,
    )
    .await
    .map_err(|_| {
        ImportAdapterError::Artifact(ArtifactError::Conflict("source_authorization_denied"))
    })?;
    Ok(controls.read_assertion_provider(context))
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
    catalog_project: Option<String>,
    repository: BTreeMap<String, Arc<dyn RepositoryConnection>>,
    import_gates: [tokio::sync::Mutex<()>; IMPORT_GATE_STRIPES],
}

impl ImportCoordinator {
    fn import_gate_index(scope: &str) -> usize {
        let digest = Sha256::digest(scope.as_bytes());
        usize::from(digest[0]) % IMPORT_GATE_STRIPES
    }

    async fn acquire_import_gate(
        &self,
        scope: &str,
        deadline: std::time::Duration,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, ImportAdapterError> {
        tokio::time::timeout(
            deadline,
            self.import_gates[Self::import_gate_index(scope)].lock(),
        )
        .await
        .map_err(|_| ArtifactError::Busy.into())
    }

    fn request_digest(
        source: &ImportSource,
        expected_library_version: u64,
        idempotency_key: &str,
    ) -> Result<String, ArtifactError> {
        let source = match source {
            ImportSource::Depot {
                connection_id,
                artifact_id,
                revision_id,
            } => serde_json::json!({
                "kind": "depot",
                "connection_id": connection_id,
                "artifact_id": artifact_id,
                "revision_id": revision_id,
            }),
            ImportSource::Repository {
                repository,
                artifact_id,
                object_id,
            } => serde_json::json!({
                "kind": "repository",
                "repository": repository,
                "artifact_id": artifact_id,
                "object_id": object_id,
            }),
        };
        labby_runtime::artifacts::canonical_json::digest(&serde_json::json!({
            "action": "artifacts.import",
            "source": source,
            "expected_library_version": expected_library_version,
            "idempotency_key": idempotency_key,
        }))
    }

    fn ensure_source_configured(&self, source: &ImportSource) -> Result<(), ImportAdapterError> {
        let configured = match source {
            ImportSource::Depot { connection_id, .. } => self.depot.contains_key(connection_id),
            ImportSource::Repository { repository, .. } => self.repository.contains_key(repository),
        };
        if configured {
            Ok(())
        } else {
            Err(ImportAdapterError::SourceUnavailable)
        }
    }

    pub(crate) fn from_host_config(
        config: &crate::config::LabConfig,
        staging_root: &Path,
    ) -> Result<Self, ArtifactError> {
        Self::from_host_config_with_env(config, staging_root, &|name| std::env::var_os(name))
    }

    fn from_host_config_with_env(
        config: &crate::config::LabConfig,
        staging_root: &Path,
        env: &impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Self, ArtifactError> {
        let mut imports = Self {
            depot: BTreeMap::new(),
            repository: BTreeMap::new(),
            catalog_project: None,
            import_gates: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        };
        let policy = match crate::dispatch::depot::manager::host_policy(&config.depot) {
            Ok(policy) => policy,
            Err(reason) => {
                tracing::warn!(
                    reason,
                    "import host policy invalid; remote sources disabled"
                );
                return Ok(imports);
            }
        };
        let mut ids = BTreeSet::new();
        for source in &config.artifacts.sources {
            if source.id != "public"
                && config
                    .depot
                    .public_read_binding
                    .as_ref()
                    .is_some_and(|binding| {
                        source.bearer_token_env.as_deref() == Some(&binding.bearer_token_env)
                    })
            {
                tracing::warn!(connection_id = %source.id, "public credential cannot authorize another source; source disabled");
                continue;
            }
            if !ids.insert(source.id.clone()) {
                imports.depot.remove(&source.id);
                imports.repository.remove(&source.id);
                tracing::warn!(connection_id = %source.id, "duplicate import connection; source disabled");
                continue;
            }
            let single = crate::config::ArtifactPreferences {
                sources: vec![source.clone()],
            };
            match Self::from_config_with_policy(&single, staging_root, env, &policy) {
                Ok(mut source_imports) => {
                    imports.depot.append(&mut source_imports.depot);
                    imports.repository.append(&mut source_imports.repository);
                }
                Err(error) => {
                    tracing::warn!(connection_id = %source.id, error = %error, "import source initialization failed; source disabled")
                }
            }
        }
        if let Some(binding) = &config.depot.public_read_binding {
            let bind = || -> Result<_, ArtifactError> {
                config
                    .depot
                    .validate_public_acquisition(&config.artifacts)
                    .map_err(ArtifactError::Conflict)?;
                let project = config
                    .depot
                    .read_project_id
                    .clone()
                    .ok_or(ArtifactError::Conflict("public_read_project_required"))?;
                let source = config
                    .artifacts
                    .sources
                    .iter()
                    .find(|source| source.id == "public")
                    .ok_or(ArtifactError::Conflict(
                        "public_acquisition_connection_required",
                    ))?;
                let token = env(&binding.bearer_token_env)
                    .ok_or(ArtifactError::Conflict(
                        "public_acquisition_credential_required",
                    ))?
                    .into_string()
                    .map_err(|_| {
                        ArtifactError::Conflict("public_acquisition_credential_not_utf8")
                    })?;
                let mut connection =
                    imports
                        .depot
                        .get("public")
                        .cloned()
                        .ok_or(ArtifactError::Conflict(
                            "public_acquisition_connection_required",
                        ))?;
                connection.bind_catalog(binding, &source.endpoint, &token, policy.clone())?;
                Ok((project, connection))
            };
            match bind() {
                Ok((project, connection)) => {
                    imports.catalog_project = Some(project);
                    imports.depot.insert("public".to_owned(), connection);
                }
                Err(error) => {
                    imports.depot.remove("public");
                    tracing::warn!(connection_id = "public", error = %error, "catalog binding unavailable; public import source disabled");
                }
            }
        }
        Ok(imports)
    }

    #[cfg(test)]
    pub(crate) fn from_config(
        config: &crate::config::ArtifactPreferences,
        staging_root: &Path,
    ) -> Result<Self, ArtifactError> {
        Self::from_config_with_env(
            config,
            staging_root,
            &|name| std::env::var_os(name),
            &BTreeMap::new(),
        )
    }

    #[cfg(test)]
    fn from_config_with_env(
        config: &crate::config::ArtifactPreferences,
        staging_root: &Path,
        env: &impl Fn(&str) -> Option<std::ffi::OsString>,
        private_hosts: &BTreeMap<String, BTreeSet<std::net::IpAddr>>,
    ) -> Result<Self, ArtifactError> {
        let policy = crate::dispatch::depot::network::NetworkPolicy {
            private_hosts: private_hosts.clone(),
            ..Default::default()
        };
        Self::from_config_with_policy(config, staging_root, env, &policy)
    }

    fn from_config_with_policy(
        config: &crate::config::ArtifactPreferences,
        staging_root: &Path,
        env: &impl Fn(&str) -> Option<std::ffi::OsString>,
        policy: &crate::dispatch::depot::network::NetworkPolicy,
    ) -> Result<Self, ArtifactError> {
        let mut depot = BTreeMap::new();
        let mut repository: BTreeMap<String, Arc<dyn RepositoryConnection>> = BTreeMap::new();
        let mut connection_ids = BTreeSet::new();
        for source in &config.sources {
            labby_runtime::artifacts::validation::validate_id(&source.id, "connection_id")?;
            if !connection_ids.insert(source.id.clone()) {
                return Err(ArtifactError::Conflict("duplicate_import_connection_id"));
            }
            let endpoint =
                url::Url::parse(&source.endpoint).map_err(|_| ArtifactError::InvalidField {
                    field: "source.endpoint",
                    reason: "invalid_url",
                })?;
            let credential = match source.bearer_token_env.as_ref() {
                Some(name) => match env(name) {
                    Some(secret) => {
                        let secret =
                            secret
                                .into_string()
                                .map_err(|_| ArtifactError::InvalidField {
                                    field: "source.bearer_token_env",
                                    reason: "credential_not_utf8",
                                })?;
                        Some(
                            labby_runtime::artifacts::provider::ArtifactSourceCredential::bearer(
                                &secret,
                            )?,
                        )
                    }
                    // A remote source may be configured before its secret is provisioned. Keep
                    // the local library available and leave only this connection unavailable,
                    // but make the operator-visible reason explicit.
                    None => {
                        tracing::warn!(
                            connection_id = %source.id,
                            env = %name,
                            "import source credential is not set; source disabled"
                        );
                        continue;
                    }
                },
                None => None,
            };
            let source_root = staging_root.join(&source.id);
            std::fs::create_dir_all(&source_root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&source_root, std::fs::Permissions::from_mode(0o700))?;
            }
            let kind = match source.kind {
                crate::config::ArtifactSourceKind::Depot => {
                    labby_runtime::artifacts::provider::ExactArtifactSource::Depot
                }
                crate::config::ArtifactSourceKind::Repository => {
                    labby_runtime::artifacts::provider::ExactArtifactSource::Repository
                }
            };
            let trusted_private_addresses = endpoint
                .host_str()
                .and_then(|host| policy.private_hosts.get(host))
                .cloned()
                .unwrap_or_default();
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
                trusted_private_addresses,
                source_root,
                Default::default(),
            )?;
            match source.kind {
                crate::config::ArtifactSourceKind::Depot => {
                    depot.insert(source.id.clone(), connection);
                }
                crate::config::ArtifactSourceKind::Repository => {
                    repository.insert(source.id.clone(), Arc::new(connection));
                }
            }
        }
        Ok(Self {
            depot,
            repository,
            catalog_project: None,
            import_gates: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        })
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
        Self {
            depot,
            repository,
            catalog_project: None,
            import_gates: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        }
    }

    // A host-bound catalog has one authorization project. Its bearer must never be
    // usable through an unrelated destination project, even for a known exact ID.
    async fn authorize_catalog_source(
        &self,
        runtime: &AccessRuntime,
        caller: &SkillLibraryCaller,
        project_id: &str,
        source: &ImportSource,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<(), ImportAdapterError> {
        let (
            Some(catalog_project),
            ImportSource::Depot {
                connection_id,
                artifact_id,
                ..
            },
        ) = (&self.catalog_project, source)
        else {
            return Ok(());
        };
        if connection_id != "public" {
            return Ok(());
        }
        if catalog_project != project_id {
            return Err(SkillLibraryDispatchError::Authorization(
                super::auth::SkillLibraryAuthorizationError::Denied,
            )
            .into());
        }
        authorize_at_boundary(
            runtime,
            caller.clone(),
            project_id,
            SkillLibraryAction::Import,
            &CanonicalArtifactId::parse(artifact_id.clone())?,
            SkillLibraryTarget::CreateForCaller,
            correlation_id,
        )
        .await
        .map_err(SkillLibraryDispatchError::Authorization)?;
        Ok(())
    }

    async fn acquire_authorized(
        &self,
        runtime: &AccessRuntime,
        caller: &SkillLibraryCaller,
        project_id: &str,
        source: ImportSource,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<ArtifactAcquisition, ImportAdapterError> {
        self.authorize_catalog_source(runtime, caller, project_id, &source, correlation_id)
            .await?;
        let headers = delegated_read_headers(runtime, caller, project_id, &source).await?;
        let acquisition = self.acquire(source.clone(), headers).await?;
        self.authorize_catalog_source(runtime, caller, project_id, &source, correlation_id)
            .await?;
        Ok(acquisition)
    }

    async fn acquire(
        &self,
        source: ImportSource,
        headers: RequestHeaders,
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
                .acquire_exact(artifact_id, revision_id, headers)
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
        let artifact_id = match &source {
            ImportSource::Depot { artifact_id, .. } => artifact_id.as_str(),
            ImportSource::Repository { artifact_id, .. } => artifact_id.as_str(),
        };
        let request_digest =
            Self::request_digest(&source, expected_library_version, &idempotency_key)?;
        self.ensure_source_configured(&source)?;
        self.authorize_catalog_source(runtime, &caller, project_id, &source, correlation_id)
            .await?;
        let _gate = self
            .acquire_import_gate(&request_digest, IMPORT_GATE_QUEUE_DEADLINE)
            .await?;
        self.authorize_catalog_source(runtime, &caller, project_id, &source, correlation_id)
            .await?;
        // Managed-source authority must be fresh after queue admission, including
        // the receipt-only path that never reaches acquisition below.
        let _delegated_headers =
            delegated_read_headers(runtime, &caller, project_id, &source).await?;
        if let Some(receipt) = service
            .replay_import(
                runtime,
                caller.clone(),
                project_id,
                artifact_id,
                request_digest.clone(),
                &idempotency_key,
                correlation_id,
            )
            .await
            .map_err(ImportAdapterError::Dispatch)?
        {
            return Ok(receipt);
        }
        let acquisition = self
            .acquire_authorized(runtime, &caller, project_id, source, correlation_id)
            .await?;
        service
            .import_acquired(
                runtime,
                caller,
                project_id,
                acquisition,
                request_digest,
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
        let source = self.resolve_selector(source)?;
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

    fn resolve_selector(&self, source: SourceSelector) -> Result<ImportSource, ImportAdapterError> {
        Ok(match source {
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
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn import_batch_selected<G: Send + Sync + 'static>(
        &self,
        service: &SkillLibraryService<G>,
        runtime: &AccessRuntime,
        caller: SkillLibraryCaller,
        project_id: &str,
        sources: Vec<SourceSelector>,
        expected_library_version: u64,
        idempotency_key: String,
        correlation_id: &SkillLibraryCorrelationId,
    ) -> Result<Value, ImportAdapterError> {
        if sources.is_empty() || sources.len() > 100 {
            return Err(ArtifactError::InvalidField {
                field: "sources",
                reason: "batch_size",
            }
            .into());
        }
        super::params::validate_idempotency_key(&idempotency_key).map_err(|reason| {
            ArtifactError::InvalidField {
                field: "idempotency_key",
                reason,
            }
        })?;
        // Validate and derive every child key before the first provider call. Long but valid
        // parent keys use a deterministic digest so the derived key remains within the contract.
        let child_keys = (0..sources.len())
            .map(|index| derive_batch_idempotency_key(&idempotency_key, index))
            .collect::<Result<Vec<_>, _>>()?;

        let mut version = expected_library_version;
        let mut items = Vec::with_capacity(sources.len());
        for (index, (source, child_key)) in sources.into_iter().zip(child_keys).enumerate() {
            let resolved_source = match self.resolve_selector(source) {
                Ok(source) => source,
                Err(error) => return Ok(batch_partial_receipt(items, version, index, &error)),
            };
            // Import and commit one item at a time. Reusing the single-item path keeps source
            // authorization, replay binding, single-flight acquisition and terminal auditing
            // identical for batch and individual requests without retaining acquired payloads.
            let value = match self
                .import(
                    service,
                    runtime,
                    caller.clone(),
                    project_id,
                    resolved_source,
                    version,
                    child_key,
                    correlation_id,
                )
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    return Ok(batch_partial_receipt(items, version, index, &error));
                }
            };
            version = value
                .get("committed_library_version")
                .and_then(Value::as_u64)
                .ok_or(SkillLibraryDispatchError::Serialization)?;
            items.push(value);
        }
        Ok(serde_json::json!({
            "items": items,
            "imported": items.len(),
            "committed_library_version": version,
            "atomic": false
        }))
    }
}

fn derive_batch_idempotency_key(parent: &str, index: usize) -> Result<String, ImportAdapterError> {
    let direct = format!("{parent}:{index}");
    let key = if direct.len() <= super::params::MAX_IDEMPOTENCY_KEY_BYTES {
        direct
    } else {
        format!(
            "batch:{}:{index}",
            hex::encode(Sha256::digest(parent.as_bytes()))
        )
    };
    super::params::validate_idempotency_key(&key).map_err(|reason| {
        ImportAdapterError::Artifact(ArtifactError::InvalidField {
            field: "idempotency_key",
            reason,
        })
    })?;
    Ok(key)
}

fn batch_partial_receipt(
    items: Vec<Value>,
    committed_library_version: u64,
    failed_index: usize,
    error: &ImportAdapterError,
) -> Value {
    let kind = match error {
        ImportAdapterError::SourceUnavailable => "source_unavailable",
        ImportAdapterError::Artifact(ArtifactError::InvalidField { .. }) => "invalid_source",
        ImportAdapterError::Artifact(ArtifactError::NotFound(_)) => "source_not_found",
        ImportAdapterError::Artifact(ArtifactError::Conflict(_)) => "source_conflict",
        ImportAdapterError::Artifact(_) => "source_error",
        ImportAdapterError::Dispatch(SkillLibraryDispatchError::Artifact(
            ArtifactError::Conflict(_),
        )) => "commit_conflict",
        ImportAdapterError::Dispatch(_) => "commit_failed",
    };
    serde_json::json!({
        "imported": items.len(),
        "items": items,
        "failed_index": failed_index,
        "error": { "kind": kind },
        "committed_library_version": committed_library_version,
        "atomic": false
    })
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
    use std::collections::BTreeSet;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_runtime::artifacts::provider::{
        ArtifactAcquisitionTransport, ArtifactFetchPolicy, ArtifactRequestHeaderProvider,
        ArtifactTransferGate, ArtifactTransportDeadlines, ArtifactTransportFuture,
        ExactArtifactProvider, ExactArtifactRequest, ExactArtifactSource,
    };
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

    #[test]
    fn host_sources_are_isolated_and_private_grants_remain_exact() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let root = tempfile::tempdir().unwrap();
        let mut config: crate::config::LabConfig = toml::from_str(
            r#"
[[artifacts.sources]]
id = "private-depot"
kind = "depot"
endpoint = "https://depot.example.com/api/artifacts/exact"
pinned_addresses = ["10.1.0.8"]
[[artifacts.sources]]
id = "public-depot"
kind = "depot"
endpoint = "https://public.example.com/api/artifacts/exact"
pinned_addresses = ["8.8.8.8"]
"#,
        )
        .unwrap();
        let imports =
            ImportCoordinator::from_host_config_with_env(&config, root.path(), &|_| None).unwrap();
        assert!(!imports.depot.contains_key("private-depot"));
        assert!(imports.depot.contains_key("public-depot"));
        config.depot.extra.insert(
            "private_hosts".to_owned(),
            toml::Value::try_from(BTreeMap::from([("depot.example.com", vec!["10.1.0.8"])]))
                .unwrap(),
        );
        let imports =
            ImportCoordinator::from_host_config_with_env(&config, root.path(), &|_| None).unwrap();
        assert!(imports.depot.contains_key("private-depot"));
        config.artifacts.sources[0].pinned_addresses = vec!["10.1.0.9".parse().unwrap()];
        let imports =
            ImportCoordinator::from_host_config_with_env(&config, root.path(), &|_| None).unwrap();
        assert!(!imports.depot.contains_key("private-depot"));
        assert!(imports.depot.contains_key("public-depot"));
        config.artifacts.sources[0].endpoint = "not a URL".to_owned();
        assert!(
            ImportCoordinator::from_host_config_with_env(&config, root.path(), &|_| None).is_ok()
        );
    }

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

    struct BarrierDepot {
        value: ArtifactAcquisition,
        calls: Arc<AtomicUsize>,
        barrier: Arc<tokio::sync::Barrier>,
    }

    impl DepotExactProvider for BarrierDepot {
        fn acquire(
            &self,
            _artifact_id: String,
            _revision_id: String,
            _headers: RequestHeaders,
        ) -> DepotFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                self.barrier.wait().await;
                Ok(self.value.clone())
            })
        }
    }

    impl DepotExactProvider for FakeDepot {
        fn acquire(
            &self,
            _artifact_id: String,
            _revision_id: String,
            _headers: RequestHeaders,
        ) -> DepotFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(self.value.clone()) })
        }
    }

    struct FakeRepository {
        value: Result<ArtifactAcquisition, &'static str>,
        calls: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct HermeticTransport {
        acquisition: ArtifactAcquisition,
    }

    impl ArtifactAcquisitionTransport for HermeticTransport {
        fn fetch<'a>(
            &'a self,
            _request: &'a ExactArtifactRequest,
            _deadlines: ArtifactTransportDeadlines,
            gate: &'a mut ArtifactTransferGate,
            _headers: Option<&'a dyn ArtifactRequestHeaderProvider>,
        ) -> ArtifactTransportFuture<'a> {
            Box::pin(async move {
                gate.observe_peer(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))?;
                for file in &self.acquisition.files {
                    let component = self
                        .acquisition
                        .interchange
                        .revision
                        .components
                        .iter()
                        .find(|component| component.path == file.path)
                        .expect("fixture component");
                    gate.begin_file(&file.path, component.size, component.digest.clone())
                        .await?;
                    gate.write_chunk(&file.bytes).await?;
                    gate.finish_file().await?;
                }
                Ok(self.acquisition.interchange.clone())
            })
        }
    }

    struct HermeticRepository {
        provider: ExactArtifactProvider<HermeticTransport>,
        calls: Arc<AtomicUsize>,
    }

    impl RepositoryConnection for HermeticRepository {
        fn acquire_exact<'a>(
            &'a self,
            repository: &'a str,
            artifact_id: &'a str,
            object_id: &'a str,
        ) -> RepositoryFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                self.provider
                    .acquire_exact(&ExactArtifactRequest {
                        source: ExactArtifactSource::Repository,
                        source_id: repository.to_owned(),
                        artifact_id: artifact_id.to_owned(),
                        revision_id: object_id.to_owned(),
                        endpoint: url::Url::parse("https://repository.invalid/v1/artifacts/exact")
                            .expect("fixture URL"),
                        credential_origin: None,
                        pinned_addresses: BTreeSet::from([IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))]),
                        trusted_private_addresses: BTreeSet::new(),
                    })
                    .await
            })
        }
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
    async fn bound_catalog_imports_check_project_before_fetch_and_revocation_before_commit() {
        struct RevokingDepot {
            value: ArtifactAcquisition,
            access: Arc<AccessStore>,
            calls: Arc<AtomicUsize>,
        }
        impl DepotExactProvider for RevokingDepot {
            fn acquire(&self, _: String, _: String, _: RequestHeaders) -> DepotFuture<'_> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    self.access.execute_test_statement(
                        "UPDATE project_memberships SET status='suspended' WHERE membership_id='member-membership'"
                    ).await.unwrap();
                    Ok(self.value.clone())
                })
            }
        }
        for batch in [false, true] {
            for mode in ["other-project", "viewer", "revoked", "allowed"] {
                let root = tempfile::tempdir().unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                }
                let access_path = root.path().join("access.db");
                let access = AccessStore::open(access_path.clone()).await.unwrap();
                let owner = VerifiedIdentity::external(
                    Authenticator::BrowserSession,
                    "https://accounts.google.com",
                    "owner-subject",
                )
                .unwrap();
                access
                    .bootstrap_owner(BootstrapOwnerInput::new(owner, "Local", "Default").unwrap())
                    .await
                    .unwrap();
                access.seed_loadout_roles_for_test().await.unwrap();
                let access = Arc::new(access);
                let runtime = AccessRuntime::initialize(access_path).await;
                let (credential, project, catalog_project) = if mode == "viewer" {
                    ("static-bearer:viewer", "viewer-project", "viewer-project")
                } else if mode == "other-project" {
                    // This caller can use the destination, but is not a member of the catalog.
                    ("static-bearer:member", "member-project", "admin-project")
                } else {
                    ("static-bearer:member", "member-project", "member-project")
                };
                let caller = SkillLibraryCaller::new(
                    VerifiedIdentity::local_credential(Authenticator::StaticBearer, credential)
                        .unwrap(),
                    ["lab".to_owned()],
                    SkillLibraryTransport::bearer(
                        super::super::auth::SkillLibrarySurface::ApiBearer,
                        true,
                    ),
                );
                let store = Arc::new(
                    labby_runtime::artifacts::ArtifactStore::new(root.path().join("artifacts"))
                        .unwrap(),
                );
                let projection: Arc<
                    dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
                > = Arc::new(ArtifactFirstPartyProjection::default());
                let initial = projection
                    .prepare(&store, &store.library_snapshot().unwrap(), None)
                    .unwrap();
                let service = SkillLibraryService::new(
                    Arc::clone(&store),
                    BoundedBlockingExecutor::new(
                        2,
                        Duration::from_secs(1),
                        Duration::from_secs(10),
                    )
                    .unwrap(),
                    Arc::new(ActivationCoordinator::new(initial, 0)),
                    projection,
                );
                let value = acquisition("private-catalog", "depot", Some("public"), None, "exact");
                let selector = SourceSelector::Depot {
                    connection_id: "public".into(),
                    artifact_id: value.interchange.descriptor.id.clone(),
                    revision_id: value.interchange.revision.id.clone(),
                };
                let calls = Arc::new(AtomicUsize::new(0));
                let provider: Arc<dyn DepotExactProvider> = if mode == "revoked" {
                    Arc::new(RevokingDepot {
                        value,
                        access: Arc::clone(&access),
                        calls: calls.clone(),
                    })
                } else {
                    Arc::new(FakeDepot {
                        value,
                        calls: calls.clone(),
                    })
                };
                let mut coordinator =
                    ImportCoordinator::new(Some(DepotConnection::fake(provider, "public")), None);
                coordinator.catalog_project = Some(catalog_project.into());
                let correlation = SkillLibraryCorrelationId::parse("protected-import").unwrap();
                let result = if batch {
                    coordinator
                        .import_batch_selected(
                            &service,
                            &runtime,
                            caller,
                            project,
                            vec![selector.clone()],
                            0,
                            "catalog-batch".into(),
                            &correlation,
                        )
                        .await
                } else {
                    coordinator
                        .import_selected(
                            &service,
                            &runtime,
                            caller,
                            project,
                            selector.clone(),
                            0,
                            "catalog-single".into(),
                            &correlation,
                        )
                        .await
                };
                let allowed = mode == "allowed";
                if batch {
                    assert_eq!(result.unwrap()["imported"], usize::from(allowed), "{mode}");
                } else {
                    assert_eq!(result.is_ok(), allowed, "{mode}: {result:?}");
                }
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    usize::from(allowed || mode == "revoked"),
                    "{mode}"
                );
                assert_eq!(
                    store.library_snapshot().unwrap().version,
                    u64::from(allowed),
                    "{mode}"
                );
                if allowed && !batch {
                    access
                        .execute_test_statement(
                            "UPDATE project_memberships SET status='suspended' WHERE membership_id='member-membership'",
                        )
                        .await
                        .unwrap();
                    let replay_after_revocation = coordinator
                        .import_selected(
                            &service,
                            &runtime,
                            SkillLibraryCaller::new(
                                VerifiedIdentity::local_credential(
                                    Authenticator::StaticBearer,
                                    "static-bearer:member",
                                )
                                .unwrap(),
                                ["lab".to_owned()],
                                SkillLibraryTransport::bearer(
                                    super::super::auth::SkillLibrarySurface::ApiBearer,
                                    true,
                                ),
                            ),
                            project,
                            selector,
                            0,
                            "catalog-single".into(),
                            &SkillLibraryCorrelationId::parse("protected-import-replay").unwrap(),
                        )
                        .await;
                    assert!(replay_after_revocation.is_err());
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        1,
                        "revoked replay must fail before another provider request"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn import_gate_queue_timeout_is_typed_busy_and_releases_cleanly() {
        let coordinator = ImportCoordinator::new(None, None);
        let scope = "same-import-scope";
        let held = coordinator.import_gates[ImportCoordinator::import_gate_index(scope)]
            .lock()
            .await;
        let saturated = coordinator
            .acquire_import_gate(scope, Duration::from_millis(1))
            .await;
        assert!(matches!(
            saturated,
            Err(ImportAdapterError::Artifact(ArtifactError::Busy))
        ));
        drop(held);
        let reacquired = coordinator
            .acquire_import_gate(scope, Duration::from_secs(1))
            .await;
        assert!(reacquired.is_ok(), "released import gate must be reusable");
    }

    #[tokio::test]
    async fn unrelated_import_scopes_do_not_share_one_global_gate() {
        let coordinator = ImportCoordinator::new(None, None);
        let first = "slow-import-scope";
        let first_index = ImportCoordinator::import_gate_index(first);
        let second = (0..1_000)
            .map(|index| format!("unrelated-import-{index}"))
            .find(|scope| ImportCoordinator::import_gate_index(scope) != first_index)
            .expect("fixed stripe set has an unrelated scope");
        let held = coordinator.import_gates[first_index].lock().await;
        let unrelated = coordinator
            .acquire_import_gate(&second, Duration::from_millis(50))
            .await;
        assert!(
            unrelated.is_ok(),
            "unrelated import must not queue behind the held stripe"
        );
        drop(held);
    }

    #[tokio::test]
    async fn independent_coordinators_converge_after_concurrent_provider_fetches() {
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
            "concurrent-owner",
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
        > = Arc::new(ArtifactFirstPartyProjection::default());
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            BoundedBlockingExecutor::new(4, Duration::from_secs(1), Duration::from_secs(10))
                .unwrap(),
            Arc::new(ActivationCoordinator::new(initial, 0)),
            projection,
        );
        let acquired = acquisition(
            "cross-coordinator",
            "depot",
            Some("shared-account"),
            None,
            "shared-object",
        );
        let artifact_id = acquired.interchange.descriptor.id.clone();
        let revision_id = acquired.interchange.revision.id.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let coordinator = || {
            ImportCoordinator::new(
                Some(DepotConnection::fake(
                    Arc::new(BarrierDepot {
                        value: acquired.clone(),
                        calls: Arc::clone(&calls),
                        barrier: Arc::clone(&barrier),
                    }),
                    "shared-account",
                )),
                None,
            )
        };
        let first = coordinator();
        let second = coordinator();
        let source = || ImportSource::Depot {
            connection_id: "shared-account".to_owned(),
            artifact_id: artifact_id.clone(),
            revision_id: revision_id.clone(),
        };
        let first_correlation = SkillLibraryCorrelationId::parse("cross-coordinator-a").unwrap();
        let second_correlation = SkillLibraryCorrelationId::parse("cross-coordinator-b").unwrap();
        let (a, b) = tokio::join!(
            first.import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                source(),
                0,
                "cross-coordinator-key".to_owned(),
                &first_correlation,
            ),
            second.import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                source(),
                0,
                "cross-coordinator-key".to_owned(),
                &second_correlation,
            )
        );
        let a = a.unwrap();
        let b = b.unwrap();
        let outcomes = [a["outcome"].as_str(), b["outcome"].as_str()];
        assert!(
            outcomes.contains(&Some("committed")),
            "outcomes: {outcomes:?}"
        );
        assert!(
            outcomes.contains(&Some("replayed")),
            "outcomes: {outcomes:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "both coordinators may fetch"
        );
        assert_eq!(store.library_snapshot().unwrap().version, 1);
    }

    #[tokio::test]
    async fn legacy_import_receipt_replays_after_source_digest_upgrade() {
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
            "legacy-owner",
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
        > = Arc::new(ArtifactFirstPartyProjection::default());
        let initial = projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let service = SkillLibraryService::new(
            Arc::clone(&store),
            BoundedBlockingExecutor::new(2, Duration::from_secs(1), Duration::from_secs(10))
                .unwrap(),
            Arc::new(ActivationCoordinator::new(initial, 0)),
            projection,
        );
        let acquired = acquisition(
            "legacy-import",
            "depot",
            Some("account-legacy"),
            None,
            "object-legacy",
        );
        let artifact_id = acquired.interchange.descriptor.id.clone();
        let revision_id = acquired.interchange.revision.id.clone();
        let key = "legacy-import-key";
        let legacy_digest = labby_runtime::artifacts::canonical_json::digest(&json!({
            "action":"artifacts.import",
            "artifact_id":artifact_id,
            "revision_id":revision_id,
            "expected_library_version":0,
            "idempotency_key":key,
        }))
        .unwrap();
        let committed = service
            .import_acquired(
                &runtime,
                caller(),
                "bootstrap-default",
                acquired.clone(),
                legacy_digest,
                0,
                key.to_owned(),
                &SkillLibraryCorrelationId::parse("legacy-import-seed").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(committed["outcome"], "committed");
        let calls = Arc::new(AtomicUsize::new(0));
        let coordinator = ImportCoordinator::new(
            Some(DepotConnection::fake(
                Arc::new(FakeDepot {
                    value: acquired,
                    calls: Arc::clone(&calls),
                }),
                "account-legacy",
            )),
            None,
        );
        let replayed = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Depot {
                    connection_id: "account-legacy".to_owned(),
                    artifact_id,
                    revision_id,
                },
                0,
                key.to_owned(),
                &SkillLibraryCorrelationId::parse("legacy-import-replay").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replayed["outcome"], "replayed");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "legacy fallback verifies exact bytes once"
        );
        assert_eq!(store.library_snapshot().unwrap().version, 1);
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
        > = Arc::new(ArtifactFirstPartyProjection::default());
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
                    "artifacts.import",
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
        let mut repository = acquisition(
            "repo-import",
            "repository",
            None,
            Some("repo-1"),
            &format!("sha256:{}", "0".repeat(64)),
        );
        let repository_revision = repository.interchange.revision.id.clone();
        repository.interchange.provenance.reference = Some(repository_revision.clone());
        repository.validate().unwrap();
        let repository_id = repository.interchange.descriptor.id.clone();
        let repository_calls = Arc::new(AtomicUsize::new(0));
        let repository_staging = root.path().join("repository-staging");
        std::fs::create_dir(&repository_staging).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&repository_staging, std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let repository_provider = ExactArtifactProvider::new(
            HermeticTransport {
                acquisition: repository,
            },
            &repository_staging,
            ArtifactFetchPolicy::default(),
        )
        .unwrap();
        let coordinator = ImportCoordinator::new(
            Some(DepotConnection::fake(
                Arc::new(FakeDepot {
                    value: depot,
                    calls: Arc::clone(&depot_calls),
                }),
                "account-1",
            )),
            Some(Arc::new(HermeticRepository {
                provider: repository_provider,
                calls: Arc::clone(&repository_calls),
            })),
        );

        let source = || SourceSelector::Depot {
            connection_id: "account-1".to_owned(),
            artifact_id: depot_id.clone(),
            revision_id: depot_revision.clone(),
        };
        let first_correlation = SkillLibraryCorrelationId::parse("depot-import-1").unwrap();
        let concurrent_correlation =
            SkillLibraryCorrelationId::parse("depot-import-concurrent").unwrap();
        let (first, concurrent) = tokio::join!(
            coordinator.import_selected(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                source(),
                0,
                "depot-import-key".to_owned(),
                &first_correlation,
            ),
            coordinator.import_selected(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                source(),
                0,
                "depot-import-key".to_owned(),
                &concurrent_correlation,
            )
        );
        let first = first.unwrap();
        let concurrent = concurrent.unwrap();
        let outcomes = [first["outcome"].as_str(), concurrent["outcome"].as_str()];
        assert!(outcomes.contains(&Some("committed")));
        assert!(outcomes.contains(&Some("replayed")));
        assert_eq!(depot_calls.load(Ordering::SeqCst), 1);
        let depot_local_id = first["artifact_id"].as_str().unwrap().to_owned();
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
        assert_eq!(
            depot_calls.load(Ordering::SeqCst),
            1,
            "a committed identical retry must use its durable receipt without refetching Depot"
        );
        assert_eq!(store.library_snapshot().unwrap().version, 1);
        let reopened_projection: Arc<
            dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>,
        > = Arc::new(ArtifactFirstPartyProjection::default());
        let reopened_initial = reopened_projection
            .prepare(&store, &store.library_snapshot().unwrap(), None)
            .unwrap();
        let reopened_service = SkillLibraryService::new(
            Arc::clone(&store),
            BoundedBlockingExecutor::new(2, Duration::from_secs(1), Duration::from_secs(10))
                .unwrap(),
            Arc::new(ActivationCoordinator::new(reopened_initial, 0)),
            reopened_projection,
        );
        let reopened_replay = coordinator
            .import(
                &reopened_service,
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
                &SkillLibraryCorrelationId::parse("depot-import-reopened").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reopened_replay["outcome"], "replayed");
        assert_eq!(
            depot_calls.load(Ordering::SeqCst),
            1,
            "durable replay must survive service reconstruction"
        );

        let repo_result = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: repository_id.clone(),
                    object_id: repository_revision.clone(),
                },
                1,
                "depot-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("repo-import-2").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(repo_result["outcome"], "committed");
        assert_eq!(repository_calls.load(Ordering::SeqCst), 1);
        let repository_local_id = repo_result["artifact_id"].as_str().unwrap().to_owned();
        let repo_replay = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: repository_id.clone(),
                    object_id: repository_revision.clone(),
                },
                1,
                "depot-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("repo-import-replay").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(repo_replay["outcome"], "replayed");
        assert_eq!(
            repository_calls.load(Ordering::SeqCst),
            1,
            "a committed repository retry must not reacquire the object"
        );
        let changed_binding = coordinator
            .import(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                ImportSource::Repository {
                    repository: "repo-1".to_owned(),
                    artifact_id: repository_id.clone(),
                    object_id: format!("sha256:{}", "f".repeat(64)),
                },
                1,
                "depot-import-key".to_owned(),
                &SkillLibraryCorrelationId::parse("repo-import-binding-change").unwrap(),
            )
            .await;
        assert!(
            matches!(
                &changed_binding,
                Err(ImportAdapterError::Artifact(ArtifactError::Conflict(
                    "provider_revision_binding_mismatch"
                )))
            ),
            "unexpected changed binding result: {changed_binding:?}"
        );
        assert_eq!(
            repository_calls.load(Ordering::SeqCst),
            2,
            "a different selector must be verified, never replay an unrelated receipt"
        );
        assert_eq!(store.library_snapshot().unwrap().records.len(), 2);
        let repository_record = store
            .library_snapshot()
            .unwrap()
            .records
            .into_values()
            .find(|record| record.artifact_id == repository_local_id)
            .expect("repository import in local library");
        assert_eq!(
            repository_record.provenance_provider.as_deref(),
            Some("repository")
        );
        let repository_list = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.list",
                json!({}),
                &SkillLibraryCorrelationId::parse("list-provenance-3").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            repository_list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["artifact_id"] == repository_local_id)
                .unwrap()["provenance"]["source"],
            "repository"
        );
        assert_eq!(std::fs::read_dir(&repository_staging).unwrap().count(), 0);
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

        let mut batch_acquisition = acquisition(
            "batch-import",
            "repository",
            None,
            Some("repo-1"),
            &format!("sha256:{}", "1".repeat(64)),
        );
        let batch_artifact_id = batch_acquisition.interchange.descriptor.id.clone();
        let batch_revision_id = batch_acquisition.interchange.revision.id.clone();
        batch_acquisition.interchange.provenance.reference = Some(batch_revision_id.clone());
        batch_acquisition.validate().unwrap();
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_coordinator = ImportCoordinator::new(
            None,
            Some(Arc::new(FakeRepository {
                value: Ok(batch_acquisition),
                calls: Arc::clone(&batch_calls),
            })),
        );
        let batch_source = SourceSelector::Repository {
            connection_id: "repo-1".to_owned(),
            artifact_id: batch_artifact_id,
            revision_id: batch_revision_id,
        };
        let batch = batch_coordinator
            .import_batch_selected(
                &service,
                &runtime,
                caller(),
                "bootstrap-default",
                vec![batch_source.clone(), batch_source],
                2,
                "x".repeat(super::super::params::MAX_IDEMPOTENCY_KEY_BYTES),
                &SkillLibraryCorrelationId::parse("batch-partial-4").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(batch["imported"], 1);
        assert_eq!(batch["items"].as_array().unwrap().len(), 1);
        assert_eq!(batch["failed_index"], 1);
        assert_eq!(batch["error"]["kind"], "commit_conflict");
        assert_eq!(batch["committed_library_version"], 3);
        assert_eq!(batch["atomic"], false);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 2);

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
                "artifacts.activate",
                json!({
                    "artifact_id": depot_local_id.clone(),
                    "expected_revision_id": depot_revision.clone(),
                    "expected_library_version": 3,
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
                "artifacts.list",
                json!({}),
                &SkillLibraryCorrelationId::parse("list-local-5").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 3);
        let get = service
            .dispatch(
                &runtime,
                caller(),
                "bootstrap-default",
                "artifacts.get",
                json!({"artifact_id": depot_local_id.clone()}),
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
                "artifacts.read",
                json!({
                    "artifact_id": depot_local_id,
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
                .acquire(
                    ImportSource::Repository {
                        repository: "repo-1".to_owned(),
                        artifact_id: "source-artifact".to_owned(),
                        object_id: "sha256:exact".to_owned(),
                    },
                    None
                )
                .await,
            Err(ImportAdapterError::Artifact(ArtifactError::Conflict(
                "source_authorization_expired"
            )))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            coordinator
                .acquire(
                    ImportSource::Repository {
                        repository: "repo-1".to_owned(),
                        artifact_id: "source-artifact".to_owned(),
                        object_id: "main".to_owned(),
                    },
                    None
                )
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
                .acquire(
                    ImportSource::Repository {
                        repository: "repo-1".to_owned(),
                        artifact_id: "source-artifact".to_owned(),
                        object_id: "sha256:exact".to_owned(),
                    },
                    None
                )
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
                .acquire(
                    ImportSource::Repository {
                        repository: "repo-1".to_owned(),
                        artifact_id: "source-artifact".to_owned(),
                        object_id: "sha256:exact".to_owned(),
                    },
                    None
                )
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
                .acquire(
                    ImportSource::Repository {
                        repository: "repo-1".to_owned(),
                        artifact_id: "source-artifact".to_owned(),
                        object_id: "sha256:exact".to_owned(),
                    },
                    None
                )
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
            &crate::config::ArtifactPreferences::default(),
            root.path(),
        )
        .unwrap();
        assert!(matches!(
            coordinator
                .acquire(
                    ImportSource::Depot {
                        connection_id: "missing".to_owned(),
                        artifact_id: "artifact".to_owned(),
                        revision_id: format!("sha256:{}", "0".repeat(64)),
                    },
                    None
                )
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
            ("depot-primary", crate::config::ArtifactSourceKind::Depot),
            (
                "repository-primary",
                crate::config::ArtifactSourceKind::Repository,
            ),
        ]
        .into_iter()
        .map(|(id, kind)| crate::config::ArtifactSourceConfig {
            id: id.to_owned(),
            kind,
            endpoint: format!("https://{id}.example/v1/exact"),
            control_plane_url: None,
            pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
            bearer_token_env: None,
        })
        .collect();
        let coordinator = ImportCoordinator::from_config(
            &crate::config::ArtifactPreferences { sources },
            root.path(),
        )
        .unwrap();
        assert!(coordinator.depot.contains_key("depot-primary"));
        assert!(coordinator.repository.contains_key("repository-primary"));
    }

    /// A source pinned to a LAN reverse proxy (e.g. SWAG terminating TLS for
    /// the Depot hostname) configures only when `[depot.private_hosts]`
    /// grants that exact address for that host.
    #[test]
    fn private_pinned_source_requires_a_depot_private_hosts_grant() {
        use std::net::{IpAddr, Ipv4Addr};

        drop(rustls::crypto::ring::default_provider().install_default());
        let root = tempfile::tempdir().unwrap();
        let lan = IpAddr::V4(Ipv4Addr::new(10, 1, 0, 8));
        let config = crate::config::ArtifactPreferences {
            sources: vec![crate::config::ArtifactSourceConfig {
                id: "public".to_owned(),
                kind: crate::config::ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/api/artifacts/exact".to_owned(),
                control_plane_url: None,
                pinned_addresses: vec![lan],
                bearer_token_env: None,
            }],
        };
        let no_env = |_: &str| None;

        assert!(matches!(
            ImportCoordinator::from_config_with_env(
                &config,
                root.path(),
                &no_env,
                &BTreeMap::new()
            ),
            Err(ArtifactError::UnsafePath("provider_dns_address"))
        ));
        let other_host = BTreeMap::from([("elsewhere.example".to_owned(), BTreeSet::from([lan]))]);
        assert!(
            ImportCoordinator::from_config_with_env(&config, root.path(), &no_env, &other_host)
                .is_err()
        );
        let granted = BTreeMap::from([("depot.example".to_owned(), BTreeSet::from([lan]))]);
        let coordinator =
            ImportCoordinator::from_config_with_env(&config, root.path(), &no_env, &granted)
                .unwrap();
        assert!(coordinator.depot.contains_key("public"));
    }

    #[test]
    fn duplicate_connection_ids_are_rejected() {
        use std::net::{IpAddr, Ipv4Addr};

        drop(rustls::crypto::ring::default_provider().install_default());
        let root = tempfile::tempdir().unwrap();
        let source = crate::config::ArtifactSourceConfig {
            id: "duplicate-source".to_owned(),
            kind: crate::config::ArtifactSourceKind::Depot,
            endpoint: "https://depot.example/v1/exact".to_owned(),
            control_plane_url: None,
            pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
            bearer_token_env: None,
        };
        let config = crate::config::ArtifactPreferences {
            sources: vec![source.clone(), source],
        };

        assert!(matches!(
            ImportCoordinator::from_config(&config, root.path()),
            Err(ArtifactError::Conflict("duplicate_import_connection_id"))
        ));
    }

    #[tokio::test]
    async fn missing_provider_credential_keeps_local_coordinator_available() {
        let root = tempfile::tempdir().unwrap();
        let missing_env = format!(
            "LABBY_TEST_MISSING_ARTIFACT_SECRET_{}_{}",
            std::process::id(),
            root.path().display()
        )
        .replace(['/', '.', '-'], "_");
        assert!(std::env::var_os(&missing_env).is_none());
        let config = crate::config::ArtifactPreferences {
            sources: vec![crate::config::ArtifactSourceConfig {
                id: "credential-pending".to_owned(),
                kind: crate::config::ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/v1/exact".to_owned(),
                control_plane_url: None,
                pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
                bearer_token_env: Some(missing_env),
            }],
        };

        let coordinator = ImportCoordinator::from_config(&config, root.path()).unwrap();
        assert!(matches!(
            coordinator
                .acquire(
                    ImportSource::Depot {
                        connection_id: "credential-pending".to_owned(),
                        artifact_id: "artifact".to_owned(),
                        revision_id: format!("sha256:{}", "0".repeat(64)),
                    },
                    None
                )
                .await,
            Err(ImportAdapterError::SourceUnavailable)
        ));
    }
}
