//! Transport-neutral Artifact provider acquisition contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::{Read as _, Write as _};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::local_io::{load_revision_files, revision_dir};
use super::model::ArtifactInterchange;
use super::store::ArtifactStore;
use super::validation;
use super::{ArtifactError, invalid};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;
use url::Url;

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

/// Optional remote source families normalized through the same Artifact acquisition contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactArtifactSource {
    Depot,
    Repository,
}

/// Exact, server-selected remote acquisition request. Moving refs and client paths are absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactArtifactRequest {
    pub source: ExactArtifactSource,
    pub source_id: String,
    pub artifact_id: String,
    pub revision_id: String,
    pub endpoint: Url,
    pub credential_origin: Option<Url>,
    pub pinned_addresses: BTreeSet<IpAddr>,
}

impl ExactArtifactRequest {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        validation::validate_id(&self.source_id, "source_id")?;
        validation::validate_id(&self.artifact_id, "artifact_id")?;
        validation::validate_reference_id(&self.revision_id, "revision_id")?;
        validate_immutable_selector(&self.revision_id)?;
        validate_remote_origin(&self.endpoint)?;
        if self.pinned_addresses.is_empty()
            || self
                .pinned_addresses
                .iter()
                .any(|address| !public_address(*address))
        {
            return Err(ArtifactError::UnsafePath("provider_dns_address"));
        }
        if let Some(origin) = &self.credential_origin {
            validate_remote_origin(origin)?;
            if origin.origin() != self.endpoint.origin() {
                return Err(ArtifactError::Conflict("credential_origin_mismatch"));
            }
        }
        Ok(())
    }
}

fn validate_immutable_selector(value: &str) -> Result<(), ArtifactError> {
    let Some(body) = value.strip_prefix("sha256:") else {
        return Err(invalid("revision_id", "immutable_digest_required"));
    };
    if body.len() != 64
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("revision_id", "immutable_digest_required"));
    }
    Ok(())
}

/// Resource budgets applied before any remote bytes enter canonical materialization.
#[derive(Debug, Clone)]
pub struct ArtifactFetchPolicy {
    pub connect_deadline: Duration,
    pub read_deadline: Duration,
    pub total_deadline: Duration,
    pub queue_deadline: Duration,
    pub max_concurrency: usize,
}

impl Default for ArtifactFetchPolicy {
    fn default() -> Self {
        Self {
            connect_deadline: Duration::from_secs(5),
            read_deadline: Duration::from_secs(15),
            total_deadline: Duration::from_mins(1),
            queue_deadline: Duration::from_secs(2),
            max_concurrency: 4,
        }
    }
}

impl ArtifactFetchPolicy {
    fn validate(&self) -> Result<(), ArtifactError> {
        if self.connect_deadline.is_zero()
            || self.read_deadline.is_zero()
            || self.total_deadline.is_zero()
            || self.queue_deadline.is_zero()
            || self.max_concurrency == 0
            || self.connect_deadline > self.total_deadline
            || self.read_deadline > self.total_deadline
        {
            return Err(invalid(
                "provider_policy",
                "invalid_deadline_or_concurrency",
            ));
        }
        Ok(())
    }
}

/// Remote transport seam. Implementations perform I/O only through the bounded transfer gate.
pub trait ArtifactAcquisitionTransport: Send + Sync {
    fn fetch<'a>(
        &'a self,
        request: &'a ExactArtifactRequest,
        deadlines: ArtifactTransportDeadlines,
        gate: &'a mut ArtifactTransferGate,
    ) -> ArtifactTransportFuture<'a>;
}

pub type ArtifactTransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ArtifactInterchange, ArtifactError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy)]
pub struct ArtifactTransportDeadlines {
    pub connect: Duration,
    pub read: Duration,
    pub total: Duration,
}

/// Secret authorization value retained only by a configured server-side transport.
#[derive(Clone)]
pub struct ArtifactSourceCredential(reqwest::header::HeaderValue);

impl std::fmt::Debug for ArtifactSourceCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ArtifactSourceCredential([REDACTED])")
    }
}

impl ArtifactSourceCredential {
    /// Construct a sensitive bearer credential. The value is never serialized or logged.
    pub fn bearer(value: &str) -> Result<Self, ArtifactError> {
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {value}"))
            .map_err(|_| invalid("provider_credential", "invalid_header_value"))?;
        value.set_sensitive(true);
        Ok(Self(value))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactHttpRequest<'a> {
    source_id: &'a str,
    artifact_id: &'a str,
    revision_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExactHttpResponse {
    interchange: ArtifactInterchange,
    components: Vec<ExactHttpComponent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExactHttpComponent {
    path: String,
    url: String,
}

/// Concrete guarded HTTP transport shared by Depot and repository acquisition.
///
/// DNS is resolved by configuration, filtered to public addresses, and pinned into the HTTP
/// client before any request. Redirect following is disabled. The exact endpoint returns bounded
/// metadata only; component bodies are streamed incrementally through [`ArtifactTransferGate`].
#[derive(Clone)]
pub struct GuardedHttpTransport {
    client: reqwest::Client,
    endpoint: Url,
    origin: url::Origin,
    pinned_addresses: BTreeSet<IpAddr>,
    credential: Option<ArtifactSourceCredential>,
}

impl GuardedHttpTransport {
    /// Build a transport for one server-selected connection and its pre-resolved public peers.
    pub fn new(
        endpoint: Url,
        pinned_addresses: BTreeSet<IpAddr>,
        credential: Option<ArtifactSourceCredential>,
        policy: &ArtifactFetchPolicy,
    ) -> Result<Self, ArtifactError> {
        policy.validate()?;
        validate_remote_origin(&endpoint)?;
        if pinned_addresses.is_empty() || pinned_addresses.iter().any(|ip| !public_address(*ip)) {
            return Err(ArtifactError::UnsafePath("provider_dns_address"));
        }
        let host = endpoint
            .host_str()
            .ok_or(ArtifactError::UnsafePath("provider_endpoint"))?;
        let port = endpoint.port_or_known_default().unwrap_or(443);
        let pinned = *pinned_addresses
            .iter()
            .next()
            .ok_or(ArtifactError::UnsafePath("provider_dns_address"))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(policy.connect_deadline)
            .timeout(policy.total_deadline)
            .resolve(host, std::net::SocketAddr::new(pinned, port))
            .build()
            .map_err(|_| ArtifactError::Conflict("provider_client_configuration"))?;
        let origin = endpoint.origin();
        Ok(Self {
            client,
            endpoint,
            origin,
            pinned_addresses,
            credential,
        })
    }

    fn request(&self, method: reqwest::Method, url: Url) -> reqwest::RequestBuilder {
        let request = self.client.request(method, url);
        match &self.credential {
            Some(credential) => {
                request.header(reqwest::header::AUTHORIZATION, credential.0.clone())
            }
            None => request,
        }
    }

    async fn bounded_json(
        response: reqwest::Response,
        limit: u64,
    ) -> Result<Vec<u8>, ArtifactError> {
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(ArtifactError::LimitExceeded {
                what: "provider_manifest",
                limit,
            });
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ArtifactError::Conflict("provider_read_failed"))?
        {
            let next = bytes.len().saturating_add(chunk.len());
            if u64::try_from(next).unwrap_or(u64::MAX) > limit {
                return Err(ArtifactError::LimitExceeded {
                    what: "provider_manifest",
                    limit,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn validate_response(&self, response: &reqwest::Response) -> Result<(), ArtifactError> {
        if response.status().is_redirection() {
            return Err(ArtifactError::Conflict("provider_redirect_rejected"));
        }
        if !response.status().is_success() {
            return Err(ArtifactError::Conflict("provider_http_failure"));
        }
        let peer = response
            .remote_addr()
            .ok_or(ArtifactError::Conflict("provider_peer_unavailable"))?
            .ip();
        if !self.pinned_addresses.contains(&peer) || !public_address(peer) {
            return Err(ArtifactError::Conflict("provider_dns_rebinding"));
        }
        Ok(())
    }
}

impl ArtifactAcquisitionTransport for GuardedHttpTransport {
    fn fetch<'a>(
        &'a self,
        request: &'a ExactArtifactRequest,
        deadlines: ArtifactTransportDeadlines,
        gate: &'a mut ArtifactTransferGate,
    ) -> ArtifactTransportFuture<'a> {
        Box::pin(async move {
            if request.endpoint != self.endpoint
                || request.pinned_addresses != self.pinned_addresses
            {
                return Err(ArtifactError::Conflict("provider_connection_mismatch"));
            }
            let mut metadata_request = self.request(reqwest::Method::POST, self.endpoint.clone());
            metadata_request = metadata_request.json(&ExactHttpRequest {
                source_id: &request.source_id,
                artifact_id: &request.artifact_id,
                revision_id: &request.revision_id,
            });
            let metadata = tokio::time::timeout(deadlines.read, metadata_request.send())
                .await
                .map_err(|_| ArtifactError::Conflict("provider_read_timeout"))?
                .map_err(|_| ArtifactError::Conflict("provider_connect_failed"))?;
            self.validate_response(&metadata)?;
            gate.observe_peer(
                metadata
                    .remote_addr()
                    .ok_or(ArtifactError::Conflict("provider_peer_unavailable"))?
                    .ip(),
            )?;
            let metadata = Self::bounded_json(metadata, validation::MAX_RECORD_JSON_BYTES).await?;
            let envelope: ExactHttpResponse = serde_json::from_slice(&metadata)
                .map_err(|_| invalid("provider_manifest", "invalid_json"))?;
            envelope.interchange.validate()?;
            if envelope.components.len() != envelope.interchange.revision.components.len() {
                return Err(invalid("provider_manifest", "component_count_mismatch"));
            }
            for component in &envelope.interchange.revision.components {
                let source = envelope
                    .components
                    .iter()
                    .find(|candidate| candidate.path == component.path)
                    .ok_or_else(|| invalid("provider_manifest", "missing_component"))?;
                validate_component_locator(&source.url)?;
                let url = self
                    .endpoint
                    .join(&source.url)
                    .map_err(|_| ArtifactError::UnsafePath("provider_component_url"))?;
                if url.origin() != self.origin || url.username() != "" || url.password().is_some() {
                    return Err(ArtifactError::UnsafePath("provider_component_url"));
                }
                gate.begin_file(&component.path, component.size, component.digest.clone())
                    .await?;
                let response = tokio::time::timeout(
                    deadlines.read,
                    self.request(reqwest::Method::GET, url).send(),
                )
                .await
                .map_err(|_| ArtifactError::Conflict("provider_read_timeout"))?
                .map_err(|_| ArtifactError::Conflict("provider_read_failed"))?;
                self.validate_response(&response)?;
                if response
                    .content_length()
                    .is_some_and(|length| length > component.size)
                {
                    return Err(ArtifactError::LimitExceeded {
                        what: "provider_file_bytes",
                        limit: validation::MAX_FILE_BYTES,
                    });
                }
                let mut response = response;
                while let Some(chunk) = tokio::time::timeout(deadlines.read, response.chunk())
                    .await
                    .map_err(|_| ArtifactError::Conflict("provider_read_timeout"))?
                    .map_err(|_| ArtifactError::Conflict("provider_read_failed"))?
                {
                    gate.write_chunk(&chunk).await?;
                }
                gate.finish_file().await?;
            }
            Ok(envelope.interchange)
        })
    }
}

fn validate_component_locator(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('?')
        || value.contains('#')
        || value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || Url::parse(value).is_ok()
    {
        return Err(ArtifactError::UnsafePath("provider_component_url"));
    }
    Ok(())
}

/// Bounded remote adapter. Successful return contains all bytes and has no live provider handle.
pub struct ExactArtifactProvider<T> {
    transport: T,
    policy: ArtifactFetchPolicy,
    permits: Arc<Semaphore>,
    staging_root: PathBuf,
}

/// Production HTTP provider with runtime-owned DNS, redirect, credential, and streaming policy.
pub type GuardedExactArtifactProvider = ExactArtifactProvider<GuardedHttpTransport>;

impl ExactArtifactProvider<GuardedHttpTransport> {
    /// Construct one non-generic configured provider for product startup wiring.
    pub fn configured_http(
        endpoint: Url,
        pinned_addresses: BTreeSet<IpAddr>,
        credential: Option<ArtifactSourceCredential>,
        staging_root: impl Into<PathBuf>,
        policy: ArtifactFetchPolicy,
    ) -> Result<Self, ArtifactError> {
        let transport = GuardedHttpTransport::new(endpoint, pinned_addresses, credential, &policy)?;
        Self::new(transport, staging_root, policy)
    }
}

impl<T> ExactArtifactProvider<T>
where
    T: ArtifactAcquisitionTransport,
{
    pub fn new(
        transport: T,
        staging_root: impl Into<PathBuf>,
        policy: ArtifactFetchPolicy,
    ) -> Result<Self, ArtifactError> {
        policy.validate()?;
        let staging_root = staging_root.into();
        validate_staging_root(&staging_root)?;
        Ok(Self {
            transport,
            permits: Arc::new(Semaphore::new(policy.max_concurrency)),
            policy,
            staging_root,
        })
    }

    pub async fn acquire_exact(
        &self,
        request: &ExactArtifactRequest,
    ) -> Result<ArtifactAcquisition, ArtifactError> {
        request.validate()?;
        let permit = tokio::time::timeout(
            self.policy.queue_deadline,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        .map_err(|_| ArtifactError::Busy)?
        .map_err(|_| ArtifactError::Busy)?;
        let mut gate = ArtifactTransferGate::new(&self.staging_root, request).await?;
        let deadlines = ArtifactTransportDeadlines {
            connect: self.policy.connect_deadline,
            read: self.policy.read_deadline,
            total: self.policy.total_deadline,
        };
        let fetch = tokio::time::timeout(
            self.policy.total_deadline,
            self.transport.fetch(request, deadlines, &mut gate),
        )
        .await;
        let interchange = match fetch {
            Ok(Ok(interchange)) => interchange,
            Ok(Err(error)) => {
                gate.abort().await;
                return Err(error);
            }
            Err(_) => {
                gate.abort().await;
                return Err(ArtifactError::Conflict("provider_timeout"));
            }
        };
        if interchange.descriptor.id != request.artifact_id
            || interchange.revision.id != request.revision_id
        {
            gate.abort().await;
            return Err(ArtifactError::Conflict(
                "provider_revision_binding_mismatch",
            ));
        }
        let files = gate.finish().await?;
        let acquisition = ArtifactAcquisition { interchange, files };
        acquisition.validate()?;
        drop(permit);
        Ok(acquisition)
    }
}

struct StagedFile {
    path: String,
    file: std::fs::File,
    expected_bytes: u64,
    expected_digest: String,
    written: u64,
    hasher: Sha256,
}

/// Transport-facing SSRF gate and incremental private staging sink.
pub struct ArtifactTransferGate {
    endpoint: Url,
    credential_origin: Option<Url>,
    pinned_addresses: BTreeSet<IpAddr>,
    connected: bool,
    staging: tokio::sync::mpsc::Sender<StagingCommand>,
}

enum StagingCommand {
    Begin {
        path: String,
        expected_bytes: u64,
        expected_digest: String,
        reply: tokio::sync::oneshot::Sender<Result<(), ArtifactError>>,
    },
    Write {
        bytes: Vec<u8>,
        reply: tokio::sync::oneshot::Sender<Result<(), ArtifactError>>,
    },
    FinishFile {
        reply: tokio::sync::oneshot::Sender<Result<(), ArtifactError>>,
    },
    Complete {
        reply: tokio::sync::oneshot::Sender<Result<Vec<ArtifactPayloadFile>, ArtifactError>>,
    },
    Abort {
        reply: tokio::sync::oneshot::Sender<()>,
    },
}

struct StagingWorker {
    directory: PathBuf,
    files: Vec<(String, PathBuf)>,
    current: Option<StagedFile>,
    total_bytes: u64,
}

impl ArtifactTransferGate {
    async fn new(root: &Path, request: &ExactArtifactRequest) -> Result<Self, ArtifactError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let directory = root.join(format!(
            ".acquire-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let (staging, mut commands) = tokio::sync::mpsc::channel(2);
        let (initialized, ready) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let worker = StagingWorker::new(directory);
            match worker {
                Ok(mut worker) => {
                    drop(initialized.send(Ok(())));
                    while let Some(command) = commands.blocking_recv() {
                        if worker.handle(command) {
                            break;
                        }
                    }
                }
                Err(error) => drop(initialized.send(Err(error))),
            }
        });
        ready
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))??;
        Ok(Self {
            endpoint: request.endpoint.clone(),
            credential_origin: request.credential_origin.clone(),
            pinned_addresses: request.pinned_addresses.clone(),
            connected: false,
            staging,
        })
    }

    /// Pin the actual peer address. A later DNS answer cannot change this connection authority.
    pub fn observe_peer(&mut self, address: IpAddr) -> Result<(), ArtifactError> {
        if !public_address(address) || !self.pinned_addresses.contains(&address) {
            return Err(ArtifactError::Conflict("provider_dns_rebinding"));
        }
        self.connected = true;
        Ok(())
    }

    /// Redirects are rejected; transports must fetch the exact server-selected endpoint.
    pub fn observe_redirect(&self, _location: &Url) -> Result<(), ArtifactError> {
        Err(ArtifactError::Conflict("provider_redirect_rejected"))
    }

    /// Credentials may be attached only to the configured same-origin endpoint.
    pub fn authorize_credentials(&self, destination: &Url) -> Result<(), ArtifactError> {
        let Some(origin) = &self.credential_origin else {
            return Err(ArtifactError::Conflict("provider_credentials_unavailable"));
        };
        if destination.origin() != origin.origin() || destination.origin() != self.endpoint.origin()
        {
            return Err(ArtifactError::Conflict("credential_forwarding_rejected"));
        }
        Ok(())
    }

    pub async fn begin_file(
        &mut self,
        path: impl Into<String>,
        expected_bytes: u64,
        expected_digest: impl Into<String>,
    ) -> Result<(), ArtifactError> {
        if !self.connected {
            return Err(ArtifactError::Conflict("provider_transfer_state"));
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.staging
            .send(StagingCommand::Begin {
                path: path.into(),
                expected_bytes,
                expected_digest: expected_digest.into(),
                reply,
            })
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?;
        result
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?
    }

    pub async fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), ArtifactError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.staging
            .send(StagingCommand::Write {
                bytes: bytes.to_vec(),
                reply,
            })
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?;
        result
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?
    }

    pub async fn finish_file(&mut self) -> Result<(), ArtifactError> {
        let (reply, result) = tokio::sync::oneshot::channel();
        self.staging
            .send(StagingCommand::FinishFile { reply })
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?;
        result
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?
    }

    async fn finish(self) -> Result<Vec<ArtifactPayloadFile>, ArtifactError> {
        if !self.connected {
            return Err(ArtifactError::Conflict("provider_partial_transfer"));
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        self.staging
            .send(StagingCommand::Complete { reply })
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?;
        result
            .await
            .map_err(|_| ArtifactError::Conflict("provider_staging_failed"))?
    }

    async fn abort(self) {
        let (reply, result) = tokio::sync::oneshot::channel();
        if self
            .staging
            .send(StagingCommand::Abort { reply })
            .await
            .is_ok()
        {
            drop(result.await);
        }
    }
}

impl StagingWorker {
    fn new(directory: PathBuf) -> Result<Self, ArtifactError> {
        std::fs::create_dir(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            directory,
            files: Vec::new(),
            current: None,
            total_bytes: 0,
        })
    }

    fn begin(
        &mut self,
        path: String,
        expected_bytes: u64,
        expected_digest: String,
    ) -> Result<(), ArtifactError> {
        if self.current.is_some() {
            return Err(ArtifactError::Conflict("provider_transfer_state"));
        }
        validation::validate_relative_path(&path)?;
        if self.files.len() >= validation::MAX_COMPONENTS
            || expected_bytes > validation::MAX_FILE_BYTES
            || self.total_bytes.saturating_add(expected_bytes) > validation::MAX_PACKAGE_BYTES
        {
            return Err(ArtifactError::LimitExceeded {
                what: "provider_payload",
                limit: validation::MAX_PACKAGE_BYTES,
            });
        }
        let staged = self.directory.join(format!("payload-{}", self.files.len()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged)?;
        self.current = Some(StagedFile {
            path,
            file,
            expected_bytes,
            expected_digest,
            written: 0,
            hasher: Sha256::new(),
        });
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), ArtifactError> {
        let current = self
            .current
            .as_mut()
            .ok_or(ArtifactError::Conflict("provider_transfer_state"))?;
        let next = current.written.saturating_add(bytes.len() as u64);
        if next > current.expected_bytes || next > validation::MAX_FILE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "provider_file_bytes",
                limit: validation::MAX_FILE_BYTES,
            });
        }
        current.file.write_all(bytes)?;
        current.hasher.update(bytes);
        current.written = next;
        Ok(())
    }

    fn finish_file(&mut self) -> Result<(), ArtifactError> {
        let current = self
            .current
            .take()
            .ok_or(ArtifactError::Conflict("provider_transfer_state"))?;
        current.file.sync_all()?;
        let digest_bytes = current.hasher.finalize();
        let mut digest = String::from("sha256:");
        use std::fmt::Write as _;
        for byte in digest_bytes {
            write!(&mut digest, "{byte:02x}").expect("writing to a String cannot fail");
        }
        if current.written != current.expected_bytes || digest != current.expected_digest {
            return Err(ArtifactError::Conflict("provider_file_digest_mismatch"));
        }
        let staged = self.directory.join(format!("payload-{}", self.files.len()));
        self.total_bytes = self.total_bytes.saturating_add(current.written);
        self.files.push((current.path, staged));
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<ArtifactPayloadFile>, ArtifactError> {
        if self.current.is_some() || self.files.is_empty() {
            return Err(ArtifactError::Conflict("provider_partial_transfer"));
        }
        let mut result = Vec::with_capacity(self.files.len());
        for (path, staged) in &self.files {
            let mut file = std::fs::File::open(staged)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            result.push(ArtifactPayloadFile {
                path: path.clone(),
                bytes,
            });
        }
        self.files.clear();
        Ok(result)
    }

    fn handle(&mut self, command: StagingCommand) -> bool {
        match command {
            StagingCommand::Begin {
                path,
                expected_bytes,
                expected_digest,
                reply,
            } => {
                drop(reply.send(self.begin(path, expected_bytes, expected_digest)));
                false
            }
            StagingCommand::Write { bytes, reply } => {
                drop(reply.send(self.write(&bytes)));
                false
            }
            StagingCommand::FinishFile { reply } => {
                drop(reply.send(self.finish_file()));
                false
            }
            StagingCommand::Complete { reply } => {
                let result = self.finish();
                drop(std::fs::remove_dir_all(&self.directory));
                drop(reply.send(result));
                true
            }
            StagingCommand::Abort { reply } => {
                drop(std::fs::remove_dir_all(&self.directory));
                let _reply_result = reply.send(());
                true
            }
        }
    }
}

impl Drop for StagingWorker {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.directory));
    }
}

fn validate_staging_root(path: &Path) -> Result<(), ArtifactError> {
    if !path.is_absolute() {
        return Err(ArtifactError::UnsafePath("provider_staging_root"));
    }
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ArtifactError::UnsafePath("provider_staging_root"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ArtifactError::UnsafePath("provider_staging_permissions"));
        }
    }
    Ok(())
}

fn validate_remote_origin(url: &Url) -> Result<(), ArtifactError> {
    let domain = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || domain.is_empty()
        || domain == "localhost"
        || domain.ends_with(".localhost")
        || domain.ends_with(".local")
        || domain.ends_with(".internal")
        || url
            .host()
            .is_some_and(|host| !matches!(host, url::Host::Domain(_)))
    {
        return Err(ArtifactError::UnsafePath("provider_endpoint"));
    }
    Ok(())
}

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 192 && b == 0 && c == 0)
                || a >= 224)
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.to_ipv4_mapped().is_some())
        }
    }
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
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::artifacts::canonical_json;
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

    #[derive(Clone, Copy)]
    enum MockBehavior {
        Good,
        Rebind,
        Redirect,
        Partial,
        Timeout,
        ForwardCredentials,
        OverLimit,
        SlowStaging,
    }

    struct MockRemote {
        acquisition: ArtifactAcquisition,
        behavior: MockBehavior,
    }

    impl ArtifactAcquisitionTransport for MockRemote {
        fn fetch<'a>(
            &'a self,
            request: &'a ExactArtifactRequest,
            _deadlines: ArtifactTransportDeadlines,
            gate: &'a mut ArtifactTransferGate,
        ) -> ArtifactTransportFuture<'a> {
            Box::pin(async move {
                if matches!(self.behavior, MockBehavior::Rebind) {
                    gate.observe_peer(IpAddr::V4(Ipv4Addr::LOCALHOST))?;
                } else {
                    gate.observe_peer(*request.pinned_addresses.iter().next().unwrap())?;
                }
                if matches!(self.behavior, MockBehavior::Redirect) {
                    gate.observe_redirect(&Url::parse("https://redirect.example/file").unwrap())?;
                }
                if matches!(self.behavior, MockBehavior::ForwardCredentials) {
                    gate.authorize_credentials(
                        &Url::parse("https://attacker.example/file").unwrap(),
                    )?;
                }
                if matches!(self.behavior, MockBehavior::OverLimit) {
                    gate.begin_file(
                        "SKILL.md",
                        validation::MAX_FILE_BYTES + 1,
                        canonical_json::sha256_bytes(b"x"),
                    )
                    .await?;
                }
                for file in &self.acquisition.files {
                    let component = self
                        .acquisition
                        .interchange
                        .revision
                        .components
                        .iter()
                        .find(|component| component.path == file.path)
                        .unwrap();
                    gate.begin_file(&file.path, component.size, component.digest.clone())
                        .await?;
                    let midpoint = file.bytes.len() / 2;
                    gate.write_chunk(&file.bytes[..midpoint]).await?;
                    if matches!(self.behavior, MockBehavior::SlowStaging) {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    if matches!(self.behavior, MockBehavior::Timeout) {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    if matches!(self.behavior, MockBehavior::Partial) {
                        return Ok(self.acquisition.interchange.clone());
                    }
                    gate.write_chunk(&file.bytes[midpoint..]).await?;
                    gate.finish_file().await?;
                }
                Ok(self.acquisition.interchange.clone())
            })
        }
    }

    async fn remote_fixture() -> ArtifactAcquisition {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(
            source.path().join("SKILL.md"),
            b"---\nname: remote\ndescription: test\n---\nbody\n",
        )
        .unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("skill", "labby", "remote"),
                source.path(),
            )
            .unwrap();
        LocalArtifactProvider::new(store)
            .acquire(
                &ArtifactProviderRequest::new(
                    record.descriptor.id,
                    Some(record.current_revision_id),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    }

    fn exact_request(
        acquisition: &ArtifactAcquisition,
        source: ExactArtifactSource,
    ) -> ExactArtifactRequest {
        ExactArtifactRequest {
            source,
            source_id: "configured-connection".to_owned(),
            artifact_id: acquisition.interchange.descriptor.id.clone(),
            revision_id: acquisition.interchange.revision.id.clone(),
            endpoint: Url::parse("https://depot.example/v1/exact").unwrap(),
            credential_origin: Some(Url::parse("https://depot.example/").unwrap()),
            pinned_addresses: BTreeSet::from([IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))]),
        }
    }

    fn private_staging() -> tempfile::TempDir {
        let root = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    #[test]
    fn immutable_selectors_component_locators_and_credentials_fail_closed() {
        assert!(validate_immutable_selector("main").is_err());
        assert!(validate_immutable_selector("sha256:ABC").is_err());
        assert!(validate_immutable_selector(&format!("sha256:{}", "a".repeat(64))).is_ok());
        for locator in [
            "https://depot.example/file",
            "/absolute",
            "../escape",
            "files/../escape",
            "files\\escape",
            "files/data?secret=1",
        ] {
            assert!(validate_component_locator(locator).is_err(), "{locator}");
        }
        assert!(validate_component_locator("components/SKILL.md").is_ok());

        let credential = ArtifactSourceCredential::bearer("top-secret").unwrap();
        let debug = format!("{credential:?}");
        assert!(!debug.contains("top-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn guarded_http_configuration_rejects_local_and_unpinned_authority() {
        let staging = private_staging();
        let policy = ArtifactFetchPolicy::default();
        assert!(
            GuardedExactArtifactProvider::configured_http(
                Url::parse("https://localhost/exact").unwrap(),
                BTreeSet::from([IpAddr::V4(Ipv4Addr::LOCALHOST)]),
                None,
                staging.path(),
                policy.clone(),
            )
            .is_err()
        );
        assert!(
            GuardedExactArtifactProvider::configured_http(
                Url::parse("https://depot.example/exact").unwrap(),
                BTreeSet::new(),
                None,
                staging.path(),
                policy,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn depot_and_repository_exact_revisions_share_one_canonical_pipeline() {
        let acquisition = remote_fixture().await;
        for source in [ExactArtifactSource::Depot, ExactArtifactSource::Repository] {
            let staging = private_staging();
            let provider = ExactArtifactProvider::new(
                MockRemote {
                    acquisition: acquisition.clone(),
                    behavior: MockBehavior::Good,
                },
                staging.path(),
                ArtifactFetchPolicy::default(),
            )
            .unwrap();
            let result = provider
                .acquire_exact(&exact_request(&acquisition, source))
                .await
                .unwrap();
            assert_eq!(result, acquisition);
            assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn ssrf_redirect_credentials_partial_and_overlimit_fail_closed_and_clean() {
        let acquisition = remote_fixture().await;
        for behavior in [
            MockBehavior::Rebind,
            MockBehavior::Redirect,
            MockBehavior::ForwardCredentials,
            MockBehavior::Partial,
            MockBehavior::OverLimit,
        ] {
            let staging = private_staging();
            let provider = ExactArtifactProvider::new(
                MockRemote {
                    acquisition: acquisition.clone(),
                    behavior,
                },
                staging.path(),
                ArtifactFetchPolicy::default(),
            )
            .unwrap();
            assert!(
                provider
                    .acquire_exact(&exact_request(&acquisition, ExactArtifactSource::Depot))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn timeout_and_cancellation_clean_private_staging_and_release_capacity() {
        let acquisition = remote_fixture().await;
        let staging = private_staging();
        let policy = ArtifactFetchPolicy {
            total_deadline: Duration::from_millis(20),
            connect_deadline: Duration::from_millis(10),
            read_deadline: Duration::from_millis(10),
            queue_deadline: Duration::from_millis(5),
            max_concurrency: 1,
        };
        let provider = Arc::new(
            ExactArtifactProvider::new(
                MockRemote {
                    acquisition: acquisition.clone(),
                    behavior: MockBehavior::Timeout,
                },
                staging.path(),
                policy,
            )
            .unwrap(),
        );
        assert!(
            provider
                .acquire_exact(&exact_request(&acquisition, ExactArtifactSource::Depot))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);

        let request = exact_request(&acquisition, ExactArtifactSource::Repository);
        let task_provider = Arc::clone(&provider);
        let task = tokio::spawn(async move { task_provider.acquire_exact(&request).await });
        tokio::task::yield_now().await;
        let saturated = provider
            .acquire_exact(&exact_request(
                &acquisition,
                ExactArtifactSource::Repository,
            ))
            .await;
        assert!(matches!(saturated, Err(ArtifactError::Busy)));
        task.abort();
        let _cancelled = task.await;
        // Staging cleanup runs as the aborted acquisition unwinds. Poll for it
        // rather than assuming a fixed delay suffices: under parallel test load
        // 5ms is not reliably enough, which made this assertion flaky.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::read_dir(staging.path()).unwrap().count() == 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "cancelled acquisition left private staging behind"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn staged_io_preserves_single_worker_heartbeat() {
        let acquisition = remote_fixture().await;
        let staging = private_staging();
        let provider = ExactArtifactProvider::new(
            MockRemote {
                acquisition: acquisition.clone(),
                behavior: MockBehavior::SlowStaging,
            },
            staging.path(),
            ArtifactFetchPolicy::default(),
        )
        .unwrap();
        let request = exact_request(&acquisition, ExactArtifactSource::Depot);
        let acquisition = provider.acquire_exact(&request);
        tokio::pin!(acquisition);
        let mut heartbeat = tokio::time::interval(Duration::from_millis(1));
        let mut ticks = 0_u32;
        loop {
            tokio::select! {
                result = &mut acquisition => {
                    result.unwrap();
                    break;
                }
                _ = heartbeat.tick() => ticks += 1,
            }
        }
        assert!(ticks >= 3, "blocking staging starved the Tokio heartbeat");
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn moving_revision_private_endpoint_and_missing_staging_fail_before_retention() {
        let acquisition = remote_fixture().await;
        let staging = private_staging();
        let provider = ExactArtifactProvider::new(
            MockRemote {
                acquisition: acquisition.clone(),
                behavior: MockBehavior::Good,
            },
            staging.path(),
            ArtifactFetchPolicy::default(),
        )
        .unwrap();
        let mut request = exact_request(&acquisition, ExactArtifactSource::Repository);
        request.revision_id = "branch-main".to_owned();
        assert!(provider.acquire_exact(&request).await.is_err());
        request.revision_id = acquisition.interchange.revision.id.clone();
        request.pinned_addresses = BTreeSet::from([IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        assert!(provider.acquire_exact(&request).await.is_err());

        let removed = private_staging();
        let removed_path = removed.path().to_path_buf();
        let provider = ExactArtifactProvider::new(
            MockRemote {
                acquisition: acquisition.clone(),
                behavior: MockBehavior::Good,
            },
            &removed_path,
            ArtifactFetchPolicy::default(),
        )
        .unwrap();
        removed.close().unwrap();
        assert!(
            provider
                .acquire_exact(&exact_request(
                    &acquisition,
                    ExactArtifactSource::Repository,
                ))
                .await
                .is_err()
        );
        assert!(!removed_path.exists());
    }
}
