//! Restricted Incus API adapter for owner-scoped Dev Containers.
//!
//! The adapter is deliberately unusable until an operator configures an HTTPS
//! endpoint, a project name, and project-restricted mutual-TLS credentials.
//! It never connects to a local Incus socket and never falls back to Incus's
//! default project.

use std::{collections::BTreeMap, future::Future, path::Path, pin::Pin, sync::Arc};

use futures::StreamExt as _;
use labby_runtime::dev_container_image_runtime::{
    ContainerImageRuntime, ImageBuildEffect, ImageBuildHandle, ImageBuildOperation,
    ImageBuildOperationResult, ImageBuildRequest, ProvisionCommand,
};
use labby_runtime::dev_container_runtime::{
    ContainerRuntime, EngineCreateRequest, EngineHandle, EngineState,
};
use reqwest::{StatusCode, Url};
use sha2::{Digest as _, Sha256};

const URL_ENV: &str = "LABBY_DEV_CONTAINER_INCUS_URL";
const PROJECT_ENV: &str = "LABBY_DEV_CONTAINER_INCUS_PROJECT";
const CLIENT_CERT_ENV: &str = "LABBY_DEV_CONTAINER_INCUS_CLIENT_CERT";
const CLIENT_KEY_ENV: &str = "LABBY_DEV_CONTAINER_INCUS_CLIENT_KEY";
const SERVER_CERT_ENV: &str = "LABBY_DEV_CONTAINER_INCUS_SERVER_CERT";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);

pub(crate) type DynDevContainerRuntime = dyn ContainerRuntime<Error = DevContainerEngineError>;
pub(crate) type DynDevContainerImageRuntime =
    dyn ContainerImageRuntime<Error = DevContainerEngineError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DevContainerEngineError {
    #[error("Dev Container engine is not configured")]
    Unconfigured,
    #[error("Dev Container engine configuration is invalid: {0}")]
    InvalidConfiguration(&'static str),
    #[error("Dev Container engine credential file is unavailable: {0}")]
    CredentialFile(&'static str),
    #[error("Dev Container engine TLS configuration is invalid")]
    InvalidTls,
    #[error("Dev Container engine client initialization failed")]
    ClientInitialization,
    #[error("Dev Container engine request failed during {operation}")]
    Transport {
        operation: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Dev Container engine response exceeded its size limit during {0}")]
    ResponseTooLarge(&'static str),
    #[error("Dev Container engine returned an invalid response during {0}")]
    InvalidResponse(&'static str),
    #[error("Dev Container engine rejected {operation} with status {status}")]
    Rejected {
        operation: &'static str,
        status: u16,
    },
    #[error("Dev Container engine resource ownership proof did not match")]
    OwnershipMismatch,
    #[error("the restricted Incus runtime does not support requested host capabilities")]
    UnsupportedHostCapability,
    #[error("Dev Container engine reported an unexpected instance state")]
    UnexpectedState,
    #[error("Dev Container Incus profile content did not match its approved digest")]
    ProfileMismatch,
}

struct UnavailableContainerRuntime(DevContainerEngineError);

impl ContainerRuntime for UnavailableContainerRuntime {
    type Error = DevContainerEngineError;

    fn create<'a>(
        &'a self,
        _: EngineCreateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn inspect<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn start<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn stop<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn destroy<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }
}

impl UnavailableContainerRuntime {
    fn error(&self) -> DevContainerEngineError {
        match self.0 {
            DevContainerEngineError::Unconfigured => DevContainerEngineError::Unconfigured,
            DevContainerEngineError::InvalidConfiguration(field) => {
                DevContainerEngineError::InvalidConfiguration(field)
            }
            _ => DevContainerEngineError::ClientInitialization,
        }
    }
}

impl ContainerImageRuntime for UnavailableContainerRuntime {
    type Error = DevContainerEngineError;

    fn launch<'a>(
        &'a self,
        _: ImageBuildRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn provision<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
        _: &'a ProvisionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn stop<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn publish<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn cleanup<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn published_image<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Option<labby_primitives::dev_container::ImageDigest>,
                        Self::Error,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Err(self.error()) })
    }

    fn discard_image<'a>(
        &'a self,
        _: &'a ImageBuildHandle,
        _: &'a labby_primitives::dev_container::ImageDigest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(self.error()) })
    }

    fn wait<'a>(
        &'a self,
        _: &'a ImageBuildOperation,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperationResult, Self::Error>> + Send + 'a>>
    {
        Box::pin(async { Err(self.error()) })
    }
}

#[cfg(feature = "proxy-testkit")]
#[derive(Default)]
struct DeterministicContainerRuntime {
    states: std::sync::Mutex<BTreeMap<String, EngineState>>,
}

#[cfg(feature = "proxy-testkit")]
impl ContainerRuntime for DeterministicContainerRuntime {
    type Error = DevContainerEngineError;

    fn create<'a>(
        &'a self,
        request: EngineCreateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.states
                .lock()
                .expect("deterministic Dev Container state lock")
                .insert(engine_name(&request.handle), EngineState::Running);
            Ok(())
        })
    }

    fn inspect<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(self
                .states
                .lock()
                .expect("deterministic Dev Container state lock")
                .get(&engine_name(handle))
                .copied()
                .unwrap_or(EngineState::Missing))
        })
    }

    fn start<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let mut states = self
                .states
                .lock()
                .expect("deterministic Dev Container state lock");
            if let Some(state) = states.get_mut(&engine_name(handle)) {
                *state = EngineState::Running;
                Ok(())
            } else {
                Err(DevContainerEngineError::OwnershipMismatch)
            }
        })
    }

    fn stop<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let mut states = self
                .states
                .lock()
                .expect("deterministic Dev Container state lock");
            if let Some(state) = states.get_mut(&engine_name(handle)) {
                *state = EngineState::Stopped;
                Ok(())
            } else {
                Err(DevContainerEngineError::OwnershipMismatch)
            }
        })
    }

    fn destroy<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            if self
                .states
                .lock()
                .expect("deterministic Dev Container state lock")
                .remove(&engine_name(handle))
                .is_some()
            {
                Ok(())
            } else {
                Err(DevContainerEngineError::OwnershipMismatch)
            }
        })
    }
}

pub(crate) fn configured_dev_container_runtime() -> Arc<DynDevContainerRuntime> {
    #[cfg(feature = "proxy-testkit")]
    if std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some() {
        return Arc::new(DeterministicContainerRuntime::default());
    }

    match IncusContainerRuntime::from_environment() {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => Arc::new(UnavailableContainerRuntime(error)),
    }
}

pub(crate) fn unavailable_dev_container_runtime() -> Arc<DynDevContainerRuntime> {
    Arc::new(UnavailableContainerRuntime(
        DevContainerEngineError::Unconfigured,
    ))
}

pub(crate) fn configured_dev_container_image_runtime() -> Arc<DynDevContainerImageRuntime> {
    match IncusContainerRuntime::from_environment() {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => Arc::new(UnavailableContainerRuntime(error)),
    }
}

pub(crate) fn unavailable_dev_container_image_runtime() -> Arc<DynDevContainerImageRuntime> {
    Arc::new(UnavailableContainerRuntime(
        DevContainerEngineError::Unconfigured,
    ))
}

struct IncusContainerRuntime {
    client: reqwest::Client,
    base_url: Url,
    project: String,
}

impl IncusContainerRuntime {
    fn from_environment() -> Result<Self, DevContainerEngineError> {
        let values = [
            std::env::var(URL_ENV).ok(),
            std::env::var(PROJECT_ENV).ok(),
            std::env::var(CLIENT_CERT_ENV).ok(),
            std::env::var(CLIENT_KEY_ENV).ok(),
            std::env::var(SERVER_CERT_ENV).ok(),
        ];
        if values.iter().all(Option::is_none) {
            return Err(DevContainerEngineError::Unconfigured);
        }
        let [url, project, client_cert, client_key, server_cert] = values;
        let base_url = parse_base_url(
            url.as_deref()
                .ok_or(DevContainerEngineError::InvalidConfiguration(URL_ENV))?,
        )?;
        let project = project
            .filter(|value| valid_name(value))
            .ok_or(DevContainerEngineError::InvalidConfiguration(PROJECT_ENV))?;
        let client_cert = read_file(
            client_cert.as_deref(),
            CLIENT_CERT_ENV,
            FileSensitivity::Public,
        )?;
        let client_key = read_file(
            client_key.as_deref(),
            CLIENT_KEY_ENV,
            FileSensitivity::Secret,
        )?;
        let server_cert = read_file(
            server_cert.as_deref(),
            SERVER_CERT_ENV,
            FileSensitivity::Public,
        )?;
        let mut identity_pem = client_cert;
        identity_pem.push(b'\n');
        identity_pem.extend_from_slice(&client_key);
        let identity = reqwest::Identity::from_pem(&identity_pem)
            .map_err(|_| DevContainerEngineError::InvalidTls)?;
        let server_cert = reqwest::Certificate::from_pem(&server_cert)
            .map_err(|_| DevContainerEngineError::InvalidTls)?;
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .tls_certs_only([server_cert])
            .identity(identity)
            .build()
            .map_err(|_| DevContainerEngineError::ClientInitialization)?;
        Ok(Self {
            client,
            base_url,
            project,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, DevContainerEngineError> {
        let mut url = self
            .base_url
            .join(path)
            .map_err(|_| DevContainerEngineError::InvalidResponse("endpoint"))?;
        url.query_pairs_mut().append_pair("project", &self.project);
        Ok(url)
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        operation: &'static str,
    ) -> Result<(StatusCode, IncusEnvelope), DevContainerEngineError> {
        let response = request
            .send()
            .await
            .map_err(|source| DevContainerEngineError::Transport { operation, source })?;
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|source| DevContainerEngineError::Transport { operation, source })?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(DevContainerEngineError::ResponseTooLarge(operation));
            }
            body.extend_from_slice(&chunk);
        }
        let envelope = serde_json::from_slice::<IncusEnvelope>(&body)
            .map_err(|_| DevContainerEngineError::InvalidResponse(operation))?;
        if !status.is_success() {
            return Ok((status, envelope));
        }
        if envelope.error_code != 0 {
            return Err(DevContainerEngineError::Rejected {
                operation,
                status: u16::try_from(envelope.error_code).unwrap_or(500),
            });
        }
        Ok((status, envelope))
    }

    async fn wait_for_operation(
        &self,
        operation_path: &str,
        operation: &'static str,
    ) -> Result<(), DevContainerEngineError> {
        if !valid_operation_path(operation_path) {
            return Err(DevContainerEngineError::InvalidResponse(operation));
        }
        let mut url = self.endpoint(&format!("{operation_path}/wait"))?;
        url.query_pairs_mut().append_pair("timeout", "60");
        let (_, envelope) = self.send(self.client.get(url), operation).await?;
        let result = serde_json::from_value::<IncusOperation>(envelope.metadata)
            .map_err(|_| DevContainerEngineError::InvalidResponse(operation))?;
        if result.status_code != 200 || !result.err.is_empty() {
            return Err(DevContainerEngineError::Rejected {
                operation,
                status: u16::try_from(result.status_code).unwrap_or(500),
            });
        }
        Ok(())
    }

    async fn wait_for_image_operation(
        &self,
        operation: &ImageBuildOperation,
    ) -> Result<ImageBuildOperationResult, DevContainerEngineError> {
        if matches!(
            operation.operation_id.as_str(),
            "observed:launch" | "observed:stop" | "observed:cleanup" | "observed:discard"
        ) {
            return Ok(ImageBuildOperationResult { image_digest: None });
        }
        if !valid_operation_path(&operation.operation_id) {
            return Err(DevContainerEngineError::InvalidResponse("image build wait"));
        }
        let mut url = self.endpoint(&format!("{}/wait", operation.operation_id))?;
        url.query_pairs_mut().append_pair("timeout", "60");
        let (_, envelope) = self.send(self.client.get(url), "image build wait").await?;
        let result = serde_json::from_value::<IncusOperation>(envelope.metadata)
            .map_err(|_| DevContainerEngineError::InvalidResponse("image build wait"))?;
        if result.status_code != 200 || !result.err.is_empty() {
            return Err(DevContainerEngineError::Rejected {
                operation: "image build wait",
                status: u16::try_from(result.status_code).unwrap_or(500),
            });
        }
        let image_digest = if operation.effect == ImageBuildEffect::Publish {
            let fingerprint = result
                .metadata
                .get("fingerprint")
                .and_then(serde_json::Value::as_str)
                .filter(|value| {
                    value.len() == 64
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                })
                .ok_or(DevContainerEngineError::InvalidResponse("publish"))?;
            Some(
                labby_primitives::dev_container::ImageDigest::new(format!("sha256:{fingerprint}"))
                    .map_err(|_| DevContainerEngineError::InvalidResponse("publish"))?,
            )
        } else {
            None
        };
        Ok(ImageBuildOperationResult { image_digest })
    }

    async fn submit(
        &self,
        request: reqwest::RequestBuilder,
        effect: ImageBuildEffect,
        operation: &'static str,
    ) -> Result<ImageBuildOperation, DevContainerEngineError> {
        let (status, envelope) = self.send(request, operation).await?;
        if !status.is_success() {
            return Err(DevContainerEngineError::Rejected {
                operation,
                status: status.as_u16(),
            });
        }
        let operation_id = envelope
            .operation
            .filter(|value| valid_operation_path(value))
            .ok_or(DevContainerEngineError::InvalidResponse(operation))?;
        Ok(ImageBuildOperation {
            effect,
            operation_id,
        })
    }

    async fn mutate(
        &self,
        request: reqwest::RequestBuilder,
        operation: &'static str,
    ) -> Result<(), DevContainerEngineError> {
        let (status, envelope) = self.send(request, operation).await?;
        if !status.is_success() {
            return Err(DevContainerEngineError::Rejected {
                operation,
                status: status.as_u16(),
            });
        }
        let operation_path = envelope
            .operation
            .ok_or(DevContainerEngineError::InvalidResponse(operation))?;
        self.wait_for_operation(&operation_path, operation).await
    }

    async fn instance(
        &self,
        handle: &EngineHandle,
    ) -> Result<Option<IncusInstance>, DevContainerEngineError> {
        let name = engine_name(handle);
        let url = self.endpoint(&format!("1.0/instances/{name}"))?;
        let (status, envelope) = self.send(self.client.get(url), "inspect").await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(DevContainerEngineError::Rejected {
                operation: "inspect",
                status: status.as_u16(),
            });
        }
        let instance = serde_json::from_value::<IncusInstance>(envelope.metadata)
            .map_err(|_| DevContainerEngineError::InvalidResponse("inspect"))?;
        verify_instance(handle, &name, &instance)?;
        Ok(Some(instance))
    }

    async fn builder_instance(
        &self,
        handle: &ImageBuildHandle,
    ) -> Result<Option<IncusInstance>, DevContainerEngineError> {
        validate_build_handle(handle)?;
        let url = self.endpoint(&format!("1.0/instances/{}", handle.builder_instance_name))?;
        let (status, envelope) = self.send(self.client.get(url), "inspect builder").await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(DevContainerEngineError::Rejected {
                operation: "inspect builder",
                status: status.as_u16(),
            });
        }
        let instance = serde_json::from_value::<IncusInstance>(envelope.metadata)
            .map_err(|_| DevContainerEngineError::InvalidResponse("inspect builder"))?;
        verify_builder(handle, &instance)?;
        Ok(Some(instance))
    }

    async fn set_state(
        &self,
        handle: &EngineHandle,
        action: &'static str,
    ) -> Result<(), DevContainerEngineError> {
        self.instance(handle)
            .await?
            .ok_or(DevContainerEngineError::OwnershipMismatch)?;
        let name = engine_name(handle);
        let url = self.endpoint(&format!("1.0/instances/{name}/state"))?;
        self.mutate(
            self.client.put(url).json(&serde_json::json!({
                "action": action,
                "timeout": 30,
                "force": false,
                "stateful": false,
            })),
            action,
        )
        .await
    }

    async fn verify_profiles(
        &self,
        profiles: &[labby_runtime::dev_container_runtime::IncusProfileReference],
    ) -> Result<Vec<String>, DevContainerEngineError> {
        if profiles.is_empty() {
            return Err(DevContainerEngineError::ProfileMismatch);
        }
        let mut names = Vec::with_capacity(profiles.len());
        for profile in profiles {
            if !valid_profile_name(&profile.name) {
                return Err(DevContainerEngineError::ProfileMismatch);
            }
            let url = self.endpoint(&format!("1.0/profiles/{}", profile.name))?;
            let (status, envelope) = self.send(self.client.get(url), "verify profile").await?;
            if !status.is_success() {
                return Err(DevContainerEngineError::ProfileMismatch);
            }
            let descriptor = serde_json::from_value::<IncusProfileDescriptor>(envelope.metadata)
                .map_err(|_| DevContainerEngineError::InvalidResponse("verify profile"))?;
            let canonical = serde_json::to_vec(&serde_json::json!({
                "config": descriptor.config,
                "devices": descriptor.devices,
            }))
            .map_err(|_| DevContainerEngineError::InvalidResponse("verify profile"))?;
            let actual = format!("sha256:{}", hex::encode(Sha256::digest(canonical)));
            if actual != profile.content_digest {
                return Err(DevContainerEngineError::ProfileMismatch);
            }
            names.push(profile.name.clone());
        }
        Ok(names)
    }
}

impl ContainerRuntime for IncusContainerRuntime {
    type Error = DevContainerEngineError;

    fn create<'a>(
        &'a self,
        request: EngineCreateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            if !request.host_capabilities.is_empty() {
                return Err(DevContainerEngineError::UnsupportedHostCapability);
            }
            let name = engine_name(&request.handle);
            let fingerprint = request
                .image_digest
                .strip_prefix("sha256:")
                .ok_or(DevContainerEngineError::InvalidResponse("create"))?;
            let cpu_count = request.cpu_millis.div_ceil(1_000);
            let profiles = self.verify_profiles(&request.profiles).await?;
            if request.launch_manifest_digest.len() != 71
                || !request.launch_manifest_digest.starts_with("sha256:")
                || request.launch_manifest_digest[7..]
                    .bytes()
                    .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
            {
                return Err(DevContainerEngineError::InvalidResponse("create"));
            }
            let mut config = BTreeMap::from([
                ("limits.cpu".to_owned(), cpu_count.to_string()),
                (
                    "limits.cpu.allowance".to_owned(),
                    format!("{}ms/1000ms", request.cpu_millis),
                ),
                (
                    "limits.memory".to_owned(),
                    format!("{}B", request.memory_bytes),
                ),
                ("limits.processes".to_owned(), "1024".to_owned()),
                ("security.nesting".to_owned(), "false".to_owned()),
                ("security.privileged".to_owned(), "false".to_owned()),
                ("user.labby.managed".to_owned(), "true".to_owned()),
                (
                    "user.labby.instance_id".to_owned(),
                    request.handle.instance_id.as_str().to_owned(),
                ),
                (
                    "user.labby.lifecycle_nonce".to_owned(),
                    request.handle.lifecycle_nonce.as_str().to_owned(),
                ),
                (
                    "user.labby.image_digest".to_owned(),
                    request.image_digest.clone(),
                ),
                (
                    "user.labby.lifetime_seconds".to_owned(),
                    request.lifetime_seconds.to_string(),
                ),
                (
                    "user.labby.launch_manifest_digest".to_owned(),
                    request.launch_manifest_digest.clone(),
                ),
            ]);
            for (name, value) in request.environment.iter() {
                if !valid_environment_name(name) {
                    return Err(DevContainerEngineError::InvalidResponse("create"));
                }
                config.insert(format!("environment.{name}"), value.to_owned());
            }
            let url = self.endpoint("1.0/instances")?;
            self.mutate(
                self.client.post(url).json(&serde_json::json!({
                    "name": name,
                    "type": "container",
                    "start": true,
                    "profiles": profiles,
                    "source": {"type": "image", "fingerprint": fingerprint},
                    "config": config,
                    "devices": {
                        "root": {
                            "type": "disk",
                            "path": "/",
                            "pool": "default",
                            "size": format!("{}B", request.disk_bytes),
                        }
                    }
                })),
                "create",
            )
            .await
        })
    }

    fn inspect<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let Some(instance) = self.instance(handle).await? else {
                return Ok(EngineState::Missing);
            };
            match instance.status.as_str() {
                "Running" => Ok(EngineState::Running),
                "Stopped" => Ok(EngineState::Stopped),
                _ => Err(DevContainerEngineError::UnexpectedState),
            }
        })
    }

    fn start<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move { self.set_state(handle, "start").await })
    }

    fn stop<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move { self.set_state(handle, "stop").await })
    }

    fn destroy<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.instance(handle)
                .await?
                .ok_or(DevContainerEngineError::OwnershipMismatch)?;
            let name = engine_name(handle);
            let mut url = self.endpoint(&format!("1.0/instances/{name}"))?;
            url.query_pairs_mut().append_pair("force", "true");
            self.mutate(self.client.delete(url), "destroy").await
        })
    }
}

impl ContainerImageRuntime for IncusContainerRuntime {
    type Error = DevContainerEngineError;

    fn launch<'a>(
        &'a self,
        request: ImageBuildRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            validate_build_handle(&request.handle)?;
            if self.builder_instance(&request.handle).await?.is_some() {
                return Ok(ImageBuildOperation {
                    effect: ImageBuildEffect::Launch,
                    operation_id: "observed:launch".to_owned(),
                });
            }
            let fingerprint = request
                .base_image
                .as_str()
                .strip_prefix("sha256:")
                .ok_or(DevContainerEngineError::InvalidResponse("build launch"))?;
            let name = &request.handle.builder_instance_name;
            let profiles = self.verify_profiles(&request.profiles).await?;
            let url = self.endpoint("1.0/instances")?;
            self.submit(
                self.client.post(url).json(&serde_json::json!({
                    "name": name,
                    "type": "container",
                    "start": true,
                    "profiles": profiles,
                    "source": {"type": "image", "fingerprint": fingerprint},
                    "config": {
                        "limits.cpu": request.cpu_millis.div_ceil(1_000).to_string(),
                        "limits.cpu.allowance": format!("{}ms/1000ms", request.cpu_millis),
                        "limits.memory": format!("{}B", request.memory_bytes),
                        "limits.processes": "1024",
                        "security.nesting": "false",
                        "security.privileged": "false",
                        "user.labby.managed": "true",
                        "user.labby.image_builder": "true",
                        "user.labby.image_build_id": request.handle.build_id,
                        "user.labby.lifecycle_nonce": request.handle.lifecycle_nonce,
                        "user.labby.base_image_digest": request.base_image.as_str(),
                    },
                    "devices": {
                        "root": {
                            "type": "disk",
                            "path": "/",
                            "pool": "default",
                            "size": format!("{}B", request.disk_bytes),
                        }
                    }
                })),
                ImageBuildEffect::Launch,
                "build launch",
            )
            .await
        })
    }

    fn provision<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
        command: &'a ProvisionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let builder = self
                .builder_instance(handle)
                .await?
                .ok_or(DevContainerEngineError::OwnershipMismatch)?;
            if builder.status != "Running" {
                return Err(DevContainerEngineError::UnexpectedState);
            }
            let url = self.endpoint(&format!(
                "1.0/instances/{}/exec",
                handle.builder_instance_name
            ))?;
            self.submit(
                self.client.post(url).json(&serde_json::json!({
                    "command": command.argv(),
                    "environment": {"DEBIAN_FRONTEND": "noninteractive"},
                    "interactive": false,
                    "record-output": false,
                    "wait-for-websocket": false,
                })),
                ImageBuildEffect::Provision,
                "build provision",
            )
            .await
        })
    }

    fn stop<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let builder = self
                .builder_instance(handle)
                .await?
                .ok_or(DevContainerEngineError::OwnershipMismatch)?;
            if builder.status == "Stopped" {
                return Ok(ImageBuildOperation {
                    effect: ImageBuildEffect::Stop,
                    operation_id: "observed:stop".to_owned(),
                });
            }
            let url = self.endpoint(&format!(
                "1.0/instances/{}/state",
                handle.builder_instance_name
            ))?;
            self.submit(
                self.client.put(url).json(&serde_json::json!({
                    "action": "stop", "timeout": 30, "force": false, "stateful": false
                })),
                ImageBuildEffect::Stop,
                "build stop",
            )
            .await
        })
    }

    fn publish<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let builder = self
                .builder_instance(handle)
                .await?
                .ok_or(DevContainerEngineError::OwnershipMismatch)?;
            if builder.status != "Stopped" {
                return Err(DevContainerEngineError::UnexpectedState);
            }
            let url = self.endpoint("1.0/images")?;
            self.submit(
                self.client.post(url).json(&serde_json::json!({
                    "source": {"type": "container", "name": handle.builder_instance_name},
                    "properties": {
                        "user.labby.managed": "true",
                        "user.labby.image_build_id": handle.build_id,
                        "user.labby.lifecycle_nonce": handle.lifecycle_nonce,
                    }
                })),
                ImageBuildEffect::Publish,
                "publish",
            )
            .await
        })
    }

    fn cleanup<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            if self.builder_instance(handle).await?.is_none() {
                return Ok(ImageBuildOperation {
                    effect: ImageBuildEffect::Cleanup,
                    operation_id: "observed:cleanup".to_owned(),
                });
            }
            let mut url =
                self.endpoint(&format!("1.0/instances/{}", handle.builder_instance_name))?;
            url.query_pairs_mut().append_pair("force", "true");
            self.submit(
                self.client.delete(url),
                ImageBuildEffect::Cleanup,
                "build cleanup",
            )
            .await
        })
    }

    fn published_image<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Option<labby_primitives::dev_container::ImageDigest>,
                        Self::Error,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            validate_build_handle(handle)?;
            let mut url = self.endpoint("1.0/images")?;
            url.query_pairs_mut().append_pair("recursion", "1");
            let (status, envelope) = self.send(self.client.get(url), "inspect images").await?;
            if !status.is_success() {
                return Err(DevContainerEngineError::Rejected {
                    operation: "inspect images",
                    status: status.as_u16(),
                });
            }
            let images = serde_json::from_value::<Vec<IncusImage>>(envelope.metadata)
                .map_err(|_| DevContainerEngineError::InvalidResponse("inspect images"))?;
            let mut matches = images
                .into_iter()
                .filter(|image| image_matches(handle, image));
            let Some(image) = matches.next() else {
                return Ok(None);
            };
            if matches.next().is_some() {
                return Err(DevContainerEngineError::OwnershipMismatch);
            }
            let digest = labby_primitives::dev_container::ImageDigest::new(format!(
                "sha256:{}",
                image.fingerprint
            ))
            .map_err(|_| DevContainerEngineError::InvalidResponse("inspect images"))?;
            Ok(Some(digest))
        })
    }

    fn discard_image<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
        image: &'a labby_primitives::dev_container::ImageDigest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let Some(observed) = self.published_image(handle).await? else {
                return Ok(ImageBuildOperation {
                    effect: ImageBuildEffect::Discard,
                    operation_id: "observed:discard".to_owned(),
                });
            };
            if &observed != image {
                return Err(DevContainerEngineError::OwnershipMismatch);
            }
            let fingerprint = image
                .as_str()
                .strip_prefix("sha256:")
                .ok_or(DevContainerEngineError::InvalidResponse("discard image"))?;
            let url = self.endpoint(&format!("1.0/images/{fingerprint}"))?;
            self.submit(
                self.client.delete(url),
                ImageBuildEffect::Discard,
                "discard image",
            )
            .await
        })
    }

    fn wait<'a>(
        &'a self,
        operation: &'a ImageBuildOperation,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperationResult, Self::Error>> + Send + 'a>>
    {
        Box::pin(async move { self.wait_for_image_operation(operation).await })
    }
}

#[derive(serde::Deserialize)]
struct IncusEnvelope {
    #[serde(default)]
    error_code: i64,
    #[serde(default)]
    metadata: serde_json::Value,
    #[serde(default)]
    operation: Option<String>,
}

#[derive(serde::Deserialize)]
struct IncusOperation {
    #[serde(default)]
    err: String,
    status_code: i64,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct IncusInstance {
    name: String,
    status: String,
    #[serde(default)]
    config: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct IncusProfileDescriptor {
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    devices: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(serde::Deserialize)]
struct IncusImage {
    fingerprint: String,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

fn engine_name(handle: &EngineHandle) -> String {
    let mut hasher = Sha256::new();
    hasher.update(handle.instance_id.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(handle.lifecycle_nonce.as_str().as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("labby-dc-{}", &digest[..24])
}

fn validate_build_handle(handle: &ImageBuildHandle) -> Result<(), DevContainerEngineError> {
    if handle.build_id.is_empty()
        || handle.build_id.len() > 128
        || handle.lifecycle_nonce.len() < 32
        || handle.lifecycle_nonce.len() > 128
        || !handle.has_valid_name()
        || !valid_name(&handle.builder_instance_name)
    {
        return Err(DevContainerEngineError::InvalidResponse(
            "image build handle",
        ));
    }
    Ok(())
}

fn verify_builder(
    handle: &ImageBuildHandle,
    instance: &IncusInstance,
) -> Result<(), DevContainerEngineError> {
    validate_build_handle(handle)?;
    let matches = instance.name == handle.builder_instance_name
        && instance
            .config
            .get("user.labby.managed")
            .map(String::as_str)
            == Some("true")
        && instance
            .config
            .get("user.labby.image_builder")
            .map(String::as_str)
            == Some("true")
        && instance
            .config
            .get("user.labby.image_build_id")
            .map(String::as_str)
            == Some(handle.build_id.as_str())
        && instance
            .config
            .get("user.labby.lifecycle_nonce")
            .map(String::as_str)
            == Some(handle.lifecycle_nonce.as_str());
    if matches {
        Ok(())
    } else {
        Err(DevContainerEngineError::OwnershipMismatch)
    }
}

fn image_matches(handle: &ImageBuildHandle, image: &IncusImage) -> bool {
    image.fingerprint.len() == 64
        && image
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && image
            .properties
            .get("user.labby.managed")
            .map(String::as_str)
            == Some("true")
        && image
            .properties
            .get("user.labby.image_build_id")
            .map(String::as_str)
            == Some(handle.build_id.as_str())
        && image
            .properties
            .get("user.labby.lifecycle_nonce")
            .map(String::as_str)
            == Some(handle.lifecycle_nonce.as_str())
}

fn verify_instance(
    handle: &EngineHandle,
    expected_name: &str,
    instance: &IncusInstance,
) -> Result<(), DevContainerEngineError> {
    let matches = instance.name == expected_name
        && instance
            .config
            .get("user.labby.managed")
            .map(String::as_str)
            == Some("true")
        && instance
            .config
            .get("user.labby.instance_id")
            .map(String::as_str)
            == Some(handle.instance_id.as_str())
        && instance
            .config
            .get("user.labby.lifecycle_nonce")
            .map(String::as_str)
            == Some(handle.lifecycle_nonce.as_str());
    if matches {
        Ok(())
    } else {
        Err(DevContainerEngineError::OwnershipMismatch)
    }
}

fn valid_operation_path(path: &str) -> bool {
    path.strip_prefix("/1.0/operations/").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_profile_name(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

fn parse_base_url(value: &str) -> Result<Url, DevContainerEngineError> {
    let mut url =
        Url::parse(value).map_err(|_| DevContainerEngineError::InvalidConfiguration(URL_ENV))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(DevContainerEngineError::InvalidConfiguration(URL_ENV));
    }
    url.set_path("/");
    Ok(url)
}

enum FileSensitivity {
    Public,
    Secret,
}

fn read_file(
    value: Option<&str>,
    field: &'static str,
    sensitivity: FileSensitivity,
) -> Result<Vec<u8>, DevContainerEngineError> {
    let path = value
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or(DevContainerEngineError::InvalidConfiguration(field))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| DevContainerEngineError::CredentialFile(field))?;
    if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 {
        return Err(DevContainerEngineError::CredentialFile(field));
    }
    #[cfg(unix)]
    if matches!(sensitivity, FileSensitivity::Secret) {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(DevContainerEngineError::CredentialFile(field));
        }
    }
    #[cfg(not(unix))]
    match sensitivity {
        FileSensitivity::Public | FileSensitivity::Secret => {}
    }
    std::fs::read(path).map_err(|_| DevContainerEngineError::CredentialFile(field))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use labby_primitives::dev_container::{DevContainerId, LifecycleNonce};
    use std::time::Duration;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, method, path, query_param},
    };

    fn handle() -> EngineHandle {
        EngineHandle {
            instance_id: DevContainerId::new("project dc").unwrap(),
            lifecycle_nonce: LifecycleNonce::new("0123456789abcdef0123456789abcdef").unwrap(),
        }
    }

    fn image_handle() -> ImageBuildHandle {
        let build_id = "build-test".to_owned();
        let lifecycle_nonce = "0123456789abcdef0123456789abcdef".to_owned();
        ImageBuildHandle {
            builder_instance_name: ImageBuildHandle::deterministic_name(
                &build_id,
                &lifecycle_nonce,
            ),
            build_id,
            lifecycle_nonce,
        }
    }

    async fn mock_runtime(timeout: Duration) -> (MockServer, IncusContainerRuntime) {
        // The workspace deliberately disables reqwest's implicit crypto
        // provider. Product startup installs ring before constructing runtime
        // state; isolated unit tests must establish the same invariant.
        drop(rustls::crypto::ring::default_provider().install_default());
        let server = MockServer::start().await;
        let runtime = IncusContainerRuntime {
            client: reqwest::Client::builder()
                .timeout(timeout)
                .no_proxy()
                .build()
                .unwrap(),
            base_url: Url::parse(&format!("{}/", server.uri())).unwrap(),
            project: "labby-dev-containers".into(),
        };
        (server, runtime)
    }

    fn profile(name: &str) -> labby_runtime::dev_container_runtime::IncusProfileReference {
        let canonical =
            serde_json::to_vec(&serde_json::json!({"config": {}, "devices": {}})).unwrap();
        labby_runtime::dev_container_runtime::IncusProfileReference {
            name: name.to_owned(),
            content_digest: format!("sha256:{}", hex::encode(Sha256::digest(canonical))),
        }
    }

    async fn expect_profile(server: &MockServer, name: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/1.0/profiles/{name}")))
            .and(query_param("project", "labby-dev-containers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error_code": 0,
                "metadata": {"config": {}, "devices": {}}
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn instance_envelope(handle: &EngineHandle, status: &str) -> serde_json::Value {
        serde_json::json!({
            "error_code": 0,
            "metadata": {
                "name": engine_name(handle),
                "status": status,
                "config": {
                    "user.labby.managed": "true",
                    "user.labby.instance_id": handle.instance_id.as_str(),
                    "user.labby.lifecycle_nonce": handle.lifecycle_nonce.as_str()
                }
            }
        })
    }

    #[test]
    fn engine_names_are_stable_safe_and_nonce_scoped() {
        let first = engine_name(&handle());
        assert!(first.starts_with("labby-dc-"));
        assert_eq!(first.len(), 33);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        );
        let mut replacement = handle();
        replacement.lifecycle_nonce =
            LifecycleNonce::new("fedcba9876543210fedcba9876543210").unwrap();
        assert_ne!(first, engine_name(&replacement));
    }

    #[test]
    fn ownership_proof_requires_all_managed_labels() {
        let handle = handle();
        let name = engine_name(&handle);
        let mut config = BTreeMap::from([
            ("user.labby.managed".into(), "true".into()),
            (
                "user.labby.instance_id".into(),
                handle.instance_id.as_str().into(),
            ),
            (
                "user.labby.lifecycle_nonce".into(),
                handle.lifecycle_nonce.as_str().into(),
            ),
        ]);
        let instance = IncusInstance {
            name: name.clone(),
            status: "Running".into(),
            config: config.clone(),
        };
        assert!(verify_instance(&handle, &name, &instance).is_ok());
        config.insert("user.labby.lifecycle_nonce".into(), "stale".into());
        let stale = IncusInstance {
            name: name.clone(),
            status: "Running".into(),
            config,
        };
        assert!(matches!(
            verify_instance(&handle, &name, &stale),
            Err(DevContainerEngineError::OwnershipMismatch)
        ));
    }

    #[test]
    fn endpoint_and_operation_inputs_are_bounded() {
        assert!(parse_base_url("https://10.99.99.1:8443").is_ok());
        for value in [
            "http://10.99.99.1:8443",
            "https://user@10.99.99.1:8443",
            "https://10.99.99.1:8443/other",
            "https://10.99.99.1:8443?project=default",
        ] {
            assert!(parse_base_url(value).is_err(), "accepted {value}");
        }
        assert!(valid_operation_path(
            "/1.0/operations/01234567-89ab-cdef-0123-456789abcdef"
        ));
        assert!(!valid_operation_path(
            "https://example.test/1.0/operations/x"
        ));
        assert!(!valid_operation_path(
            "/1.0/operations/../instances/default"
        ));
    }

    #[tokio::test]
    async fn create_uses_project_pinned_image_limits_and_waits_for_completion() {
        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        let handle = handle();
        expect_profile(&server, "labby-runtime-web").await;
        Mock::given(method("POST"))
            .and(path("/1.0/instances"))
            .and(query_param("project", "labby-dev-containers"))
            .and(body_partial_json(serde_json::json!({
                "name": engine_name(&handle),
                "type": "container",
                "start": true,
                "profiles": ["labby-runtime-web"],
                "source": {
                    "type": "image",
                    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "config": {
                    "limits.cpu": "1",
                    "limits.cpu.allowance": "500ms/1000ms",
                    "limits.memory": "1048576B",
                    "limits.processes": "1024",
                    "security.nesting": "false",
                    "security.privileged": "false",
                    "user.labby.managed": "true",
                    "user.labby.instance_id": handle.instance_id.as_str(),
                    "user.labby.lifecycle_nonce": handle.lifecycle_nonce.as_str(),
                    "user.labby.launch_manifest_digest": format!("sha256:{}", "c".repeat(64)),
                    "environment.REGISTRY_TOKEN": "secret-value"
                },
                "devices": {
                    "root": {"type": "disk", "path": "/", "pool": "default", "size": "2097152B"}
                }
            })))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "error_code": 0,
                "metadata": {},
                "operation": "/1.0/operations/01234567-89ab-cdef-0123-456789abcdef"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/1.0/operations/01234567-89ab-cdef-0123-456789abcdef/wait",
            ))
            .and(query_param("project", "labby-dev-containers"))
            .and(query_param("timeout", "60"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error_code": 0,
                "metadata": {"status_code": 200, "err": ""}
            })))
            .expect(1)
            .mount(&server)
            .await;

        runtime
            .create(EngineCreateRequest {
                handle,
                image_digest: format!("sha256:{}", "a".repeat(64)),
                cpu_millis: 500,
                memory_bytes: 1_048_576,
                disk_bytes: 2_097_152,
                lifetime_seconds: 3_600,
                host_capabilities: BTreeSet::new(),
                launch_manifest_digest: format!("sha256:{}", "c".repeat(64)),
                profiles: vec![profile("labby-runtime-web")],
                environment: labby_runtime::dev_container_runtime::ResolvedLaunchEnvironment::new(
                    BTreeMap::from([("REGISTRY_TOKEN".to_owned(), "secret-value".to_owned())]),
                ),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_refuses_profile_drift_before_the_engine_effect() {
        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        expect_profile(&server, "labby-runtime-web").await;
        let result = runtime
            .create(EngineCreateRequest {
                handle: handle(),
                image_digest: format!("sha256:{}", "a".repeat(64)),
                cpu_millis: 500,
                memory_bytes: 1_048_576,
                disk_bytes: 2_097_152,
                lifetime_seconds: 3_600,
                host_capabilities: BTreeSet::new(),
                launch_manifest_digest: format!("sha256:{}", "c".repeat(64)),
                profiles: vec![
                    labby_runtime::dev_container_runtime::IncusProfileReference {
                        name: "labby-runtime-web".to_owned(),
                        content_digest: format!("sha256:{}", "f".repeat(64)),
                    },
                ],
                environment:
                    labby_runtime::dev_container_runtime::ResolvedLaunchEnvironment::default(),
            })
            .await;
        assert!(matches!(
            result,
            Err(DevContainerEngineError::ProfileMismatch)
        ));
    }

    #[tokio::test]
    async fn image_builder_uses_only_operator_catalog_profiles() {
        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        let handle = image_handle();
        expect_profile(&server, "labby-build-web").await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/1.0/instances/{}",
                handle.builder_instance_name
            )))
            .and(query_param("project", "labby-dev-containers"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error_code": 0,
                "metadata": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/1.0/instances"))
            .and(query_param("project", "labby-dev-containers"))
            .and(body_partial_json(serde_json::json!({
                "name": handle.builder_instance_name.clone(),
                "profiles": ["labby-build-web"],
                "devices": {
                    "root": {
                        "type": "disk",
                        "path": "/",
                        "pool": "default",
                        "size": "2097152B"
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "error_code": 0,
                "metadata": {},
                "operation": "/1.0/operations/01234567-89ab-cdef-0123-456789abcdef"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let operation = runtime
            .launch(ImageBuildRequest {
                handle,
                base_image: labby_primitives::dev_container::ImageDigest::new(format!(
                    "sha256:{}",
                    "a".repeat(64)
                ))
                .unwrap(),
                cpu_millis: 500,
                memory_bytes: 1_048_576,
                disk_bytes: 2_097_152,
                profiles: vec![profile("labby-build-web")],
            })
            .await
            .unwrap();
        assert_eq!(operation.effect, ImageBuildEffect::Launch);
    }

    #[tokio::test]
    async fn inspect_requires_project_and_matching_ownership_labels() {
        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        let handle = handle();
        Mock::given(method("GET"))
            .and(path(format!("/1.0/instances/{}", engine_name(&handle))))
            .and(query_param("project", "labby-dev-containers"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(instance_envelope(&handle, "Running")),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            runtime.inspect(&handle).await.unwrap(),
            EngineState::Running
        );

        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        let mut response = instance_envelope(&handle, "Running");
        response["metadata"]["config"]["user.labby.lifecycle_nonce"] = serde_json::json!("stale");
        Mock::given(method("GET"))
            .and(path(format!("/1.0/instances/{}", engine_name(&handle))))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
        assert!(matches!(
            runtime.inspect(&handle).await,
            Err(DevContainerEngineError::OwnershipMismatch)
        ));
    }

    #[tokio::test]
    async fn engine_errors_and_timeouts_are_typed() {
        let handle = handle();
        let (server, runtime) = mock_runtime(Duration::from_secs(2)).await;
        Mock::given(method("GET"))
            .and(path(format!("/1.0/instances/{}", engine_name(&handle))))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error_code": 403,
                "metadata": {}
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            runtime.inspect(&handle).await,
            Err(DevContainerEngineError::Rejected {
                operation: "inspect",
                status: 403
            })
        ));

        let (server, runtime) = mock_runtime(Duration::from_millis(10)).await;
        Mock::given(method("GET"))
            .and(path(format!("/1.0/instances/{}", engine_name(&handle))))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(instance_envelope(&handle, "Running")),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            runtime.inspect(&handle).await,
            Err(DevContainerEngineError::Transport {
                operation: "inspect",
                ..
            })
        ));
    }
}
