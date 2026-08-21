//! Transport-neutral Artifact provider acquisition contracts.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use super::local_io::{load_revision_files, revision_dir};
use super::model::ArtifactInterchange;
use super::store::ArtifactStore;
use super::validation;
use super::{ArtifactError, invalid};

/// Exact Artifact revision selector understood by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactProviderRequest {
    /// Canonical source Artifact identity.
    pub artifact_id: String,
    /// Optional exact revision. Omit to ask a provider for its current head.
    pub revision_id: Option<String>,
}

impl ArtifactProviderRequest {
    /// Build and validate a provider request.
    pub fn new(
        artifact_id: impl Into<String>,
        revision_id: Option<String>,
    ) -> Result<Self, ArtifactError> {
        let request = Self {
            artifact_id: artifact_id.into(),
            revision_id,
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate provider-independent request fields.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        validation::validate_id(&self.artifact_id, "artifact_id")?;
        if let Some(revision_id) = self.revision_id.as_deref() {
            validation::validate_reference_id(revision_id, "revision_id")?;
        }
        Ok(())
    }
}

/// One acquired file payload. Bytes are intentionally not a wire DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactPayloadFile {
    /// Normalized package-relative path.
    pub path: String,
    /// Exact payload bytes.
    pub bytes: Vec<u8>,
}

/// Exact provider result: canonical metadata plus verified revision bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactAcquisition {
    /// Frozen portable Artifact metadata/revision envelope.
    pub interchange: ArtifactInterchange,
    /// Exact file payloads corresponding one-to-one with revision components.
    pub files: Vec<ArtifactPayloadFile>,
}

impl ArtifactAcquisition {
    /// Validate the interchange contract and every acquired byte payload.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        self.interchange.validate()?;
        if self.files.len() != self.interchange.revision.components.len() {
            return Err(invalid("provider_files", "component_count_mismatch"));
        }
        validate_payload_sizes(self.files.iter().map(|file| file.bytes.len()))?;

        let mut payloads = BTreeMap::new();
        for file in &self.files {
            validation::validate_relative_path(&file.path)?;
            if payloads.insert(file.path.as_str(), file).is_some() {
                return Err(invalid("provider_files", "duplicate_path"));
            }
        }

        for component in &self.interchange.revision.components {
            if component.kind != "file" {
                return Err(invalid("component_kind", "unsupported_materialization"));
            }
            let file = payloads
                .get(component.path.as_str())
                .ok_or_else(|| invalid("provider_files", "missing_component"))?;
            let size =
                u64::try_from(file.bytes.len()).map_err(|_| ArtifactError::LimitExceeded {
                    what: "file_size",
                    limit: validation::MAX_FILE_BYTES,
                })?;
            if size != component.size {
                return Err(ArtifactError::Conflict("provider_file_size_mismatch"));
            }
            if super::canonical_json::sha256_bytes(&file.bytes) != component.digest {
                return Err(ArtifactError::Conflict("provider_file_digest_mismatch"));
            }
        }
        Ok(())
    }
}

fn validate_payload_sizes(sizes: impl IntoIterator<Item = usize>) -> Result<(), ArtifactError> {
    let mut total = 0_u64;
    for size in sizes {
        let size = u64::try_from(size).map_err(|_| ArtifactError::LimitExceeded {
            what: "file_size",
            limit: validation::MAX_FILE_BYTES,
        })?;
        if size > validation::MAX_FILE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "file_size",
                limit: validation::MAX_FILE_BYTES,
            });
        }
        total = total
            .checked_add(size)
            .ok_or(ArtifactError::LimitExceeded {
                what: "package_size",
                limit: validation::MAX_PACKAGE_BYTES,
            })?;
        if total > validation::MAX_PACKAGE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "package_size",
                limit: validation::MAX_PACKAGE_BYTES,
            });
        }
    }
    Ok(())
}

/// Boxed future returned by provider adapters without requiring an async-trait dependency.
pub type ArtifactProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ArtifactAcquisition, ArtifactError>> + Send + 'a>>;

/// Provider acquisition seam. Providers fetch exact revisions but never mutate local state.
pub trait ArtifactProvider: Send + Sync {
    /// Stable provider family label used for diagnostics/configuration.
    fn name(&self) -> &'static str;

    /// Acquire one exact revision and its bytes.
    fn acquire<'a>(&'a self, request: &'a ArtifactProviderRequest) -> ArtifactProviderFuture<'a>;
}

/// Local-store implementation of the provider seam.
#[derive(Debug, Clone)]
pub struct LocalArtifactProvider {
    store: ArtifactStore,
}

impl LocalArtifactProvider {
    /// Wrap an existing local Artifact store as a provider.
    #[must_use]
    pub fn new(store: ArtifactStore) -> Self {
        Self { store }
    }
}

impl ArtifactProvider for LocalArtifactProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    fn acquire<'a>(&'a self, request: &'a ArtifactProviderRequest) -> ArtifactProviderFuture<'a> {
        Box::pin(async move {
            request.validate()?;
            let interchange = self
                .store
                .interchange(&request.artifact_id, request.revision_id.as_deref())?;
            let artifact_dir = self.store.artifact_dir(&request.artifact_id)?;
            let files = load_revision_files(
                &revision_dir(&artifact_dir, &interchange.revision.id).join("files"),
                &interchange.revision.components,
            )?
            .into_iter()
            .map(|file| ArtifactPayloadFile {
                path: file.path,
                bytes: file.bytes,
            })
            .collect();
            let acquisition = ArtifactAcquisition { interchange, files };
            acquisition.validate()?;
            Ok(acquisition)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::store::ArtifactImportRequest;
    use tempfile::tempdir;

    #[tokio::test]
    async fn local_provider_acquires_exact_verified_revision() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "provider-demo"),
                source.path(),
            )
            .unwrap();
        let provider = LocalArtifactProvider::new(store.clone());
        let request = ArtifactProviderRequest::new(
            record.descriptor.id.clone(),
            Some(record.current_revision_id.clone()),
        )
        .unwrap();
        let acquisition = provider.acquire(&request).await.unwrap();
        assert_eq!(
            acquisition.interchange.revision.id,
            record.current_revision_id
        );
        assert_eq!(acquisition.files[0].bytes, b"alpha");
    }

    #[test]
    fn provider_payload_budgets_reject_oversized_files_and_packages() {
        let too_large = usize::try_from(validation::MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            validate_payload_sizes([too_large]),
            Err(ArtifactError::LimitExceeded {
                what: "file_size",
                ..
            })
        ));

        let max_file = usize::try_from(validation::MAX_FILE_BYTES).unwrap();
        assert!(matches!(
            validate_payload_sizes([max_file; 5]),
            Err(ArtifactError::LimitExceeded {
                what: "package_size",
                ..
            })
        ));
    }

    #[test]
    fn acquisition_rejects_tampered_provider_bytes() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "tamper-demo"),
                source.path(),
            )
            .unwrap();
        let interchange = store.interchange(&record.descriptor.id, None).unwrap();
        let mut acquisition = ArtifactAcquisition {
            interchange,
            files: vec![ArtifactPayloadFile {
                path: "a.txt".to_string(),
                bytes: b"tampered".to_vec(),
            }],
        };
        assert!(matches!(
            acquisition.validate(),
            Err(ArtifactError::Conflict(
                "provider_file_size_mismatch" | "provider_file_digest_mismatch"
            ))
        ));
        acquisition.files[0].bytes = b"alpha".to_vec();
        acquisition.validate().unwrap();
    }
}
