//! Server-held remote Artifact authority used by the public control-plane services.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use labby_apis::artifact_control::{ArtifactControlClient, Operation};
use labby_apis::core::{ApiError, Auth, HttpClient};
use labby_auth::VerifiedIdentity;
use labby_auth::depot_delegation::{
    ASSERTION_AUDIENCE, ASSERTION_ISSUER, DelegatedAuthorityEpochs, DepotDelegationClaims,
    DepotDelegationSigner,
};
use labby_primitives::action::{ActionSpec, ParamSpec};
use labby_runtime::artifacts::provider::ArtifactRequestHeaderProvider;
use serde_json::Value;

use crate::config::{ArtifactPreferences, ArtifactSourceKind};
use crate::dispatch::error::ToolError;

const REMOTE_CONNECTION: ParamSpec = ParamSpec {
    name: "connection_id",
    ty: "string",
    required: false,
    description: "Configured remote Artifact authority; optional when exactly one is configured",
};
const REMOTE_CURSOR: ParamSpec = ParamSpec {
    name: "cursor",
    ty: "string",
    required: false,
    description: "Opaque remote continuation cursor",
};
const REMOTE_LIMIT: ParamSpec = ParamSpec {
    name: "limit",
    ty: "integer",
    required: false,
    description: "Bounded remote page size",
};

pub(crate) const CALLBACK_REMOTE_ACTIONS: [ActionSpec; 4] = [
    ActionSpec {
        name: "artifacts.search_remote",
        description: "Search the configured remote Artifact catalog",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifactSearch",
        params: &[
            REMOTE_CONNECTION,
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Case-insensitive remote catalog query",
            },
            REMOTE_LIMIT,
        ],
    },
    ActionSpec {
        name: "artifacts.list_remote",
        description: "List the combined hosted and projected remote Artifact catalog",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifactPage",
        params: &[REMOTE_CONNECTION, REMOTE_CURSOR, REMOTE_LIMIT],
    },
    ActionSpec {
        name: "artifacts.get_remote",
        description: "Get one remote Artifact by stable identifier",
        destructive: false,
        requires_admin: false,
        returns: "RemoteArtifact",
        params: &[
            REMOTE_CONNECTION,
            ParamSpec {
                name: "id",
                ty: "string",
                required: true,
                description: "Stable item identifier",
            },
        ],
    },
    ActionSpec {
        name: "artifacts.list_candidates",
        description: "List remote discovery candidates awaiting intake",
        destructive: false,
        requires_admin: true,
        returns: "ArtifactCandidatePage",
        params: &[REMOTE_CONNECTION, REMOTE_CURSOR, REMOTE_LIMIT],
    },
];

#[derive(Clone)]
struct AuthorityRevalidation {
    store: crate::access::AccessStore,
    identity: VerifiedIdentity,
}

#[derive(Clone)]
pub(crate) struct AuthorityContext {
    pub actor_id: String,
    pub organization_id: String,
    pub team_id: Option<String>,
    pub project_id: String,
    pub platform_administrator: bool,
    /// Exact permission revalidated for this request. Delegation must never
    /// mint a capability outside this local authorization ceiling.
    pub permission: crate::access::Permission,
    pub epochs: DelegatedAuthorityEpochs,
    revalidation: Option<AuthorityRevalidation>,
}

impl AuthorityContext {
    async fn revalidate(&self) -> Result<Self, ToolError> {
        let Some(revalidation) = self.revalidation.as_ref() else {
            return Ok(self.clone());
        };
        authorize_authority_context_with_store(
            revalidation.store.clone(),
            revalidation.identity.clone(),
            &self.project_id,
            self.team_id.as_deref(),
            self.permission,
        )
        .await
    }
}

pub(crate) async fn authorize_authority_context(
    runtime: &crate::access::AccessRuntime,
    identity: VerifiedIdentity,
    project_id: &str,
    selected_team_id: Option<&str>,
    permission: crate::access::Permission,
) -> Result<AuthorityContext, ToolError> {
    let store = runtime
        .store()
        .await
        .map_err(|error| crate::dispatch::access_errors::map_runtime_error("artifacts", error))?;
    authorize_authority_context_with_store(
        store,
        identity,
        project_id,
        selected_team_id,
        permission,
    )
    .await
}

async fn authorize_authority_context_with_store(
    store: crate::access::AccessStore,
    identity: VerifiedIdentity,
    project_id: &str,
    selected_team_id: Option<&str>,
    permission: crate::access::Permission,
) -> Result<AuthorityContext, ToolError> {
    // Permission and the delegation epoch vector must come from the same
    // transaction. Otherwise a role downgrade between separate reads could be
    // captured as a fresh epoch vector while retaining an earlier permission.
    let snapshot = store
        .depot_delegation_authority(
            identity.clone(),
            project_id.to_owned(),
            selected_team_id.map(str::to_owned),
            permission,
        )
        .await
        .map_err(|error| {
            crate::dispatch::access_errors::map_store_error("artifacts", error, || {
                ToolError::Forbidden {
                    message: "Remote Artifact operation is not authorized for this project or selected team"
                        .to_owned(),
                    required_scopes: vec!["lab:read".to_owned()],
                }
            })
        })?;
    Ok(AuthorityContext {
        actor_id: snapshot.principal_id,
        organization_id: snapshot.organization_id,
        team_id: snapshot.team_id,
        project_id: snapshot.project_id,
        platform_administrator: snapshot.platform_administrator,
        permission,
        epochs: DelegatedAuthorityEpochs {
            authority_schema: snapshot.authority_schema,
            organization_policy: snapshot.organization_policy,
            team_membership: snapshot.team_membership,
            team_policy: snapshot.team_policy,
            project_membership: snapshot.project_membership,
            project_policy: Some(snapshot.project_policy),
            global_revision: snapshot.global_revision,
        },
        revalidation: Some(AuthorityRevalidation { store, identity }),
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactControlPlane {
    clients: BTreeMap<String, AuthorityConnection>,
    delegation: Option<Arc<DelegationConfiguration>>,
}

struct DelegationConfiguration {
    signer: DepotDelegationSigner,
    deployment_id: String,
    /// Operator kill switch for managed authority. Mutations are refused while
    /// it is set; reads keep flowing so a stale projection stays diagnosable.
    managed_authority_kill_switch: bool,
}

impl std::fmt::Debug for DelegationConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationConfiguration")
            .field("deployment_id", &self.deployment_id)
            .field(
                "managed_authority_kill_switch",
                &self.managed_authority_kill_switch,
            )
            .field("signer", &self.signer)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct AuthorityConnection {
    control_plane_url: String,
    pinned_addresses: Vec<IpAddr>,
    bearer_token_env: Option<String>,
    permits: Arc<tokio::sync::Semaphore>,
}

/// Exact body binding for a delegated `POST` assertion: the SHA-256 of the
/// bytes that will be sent and their length.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyBinding {
    content_digest: String,
    content_length: u64,
}

impl BodyBinding {
    fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        Self {
            content_digest: format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes))),
            content_length: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }
    }
}

impl ArtifactControlPlane {
    #[cfg(test)]
    pub(crate) fn from_config(config: &ArtifactPreferences) -> Result<Self, ToolError> {
        Self::from_configs(config, &crate::config::depot::DepotPreferences::default())
    }

    pub(crate) fn from_configs(
        config: &ArtifactPreferences,
        depot: &crate::config::depot::DepotPreferences,
    ) -> Result<Self, ToolError> {
        if config.sources.iter().any(|source| {
            source.kind == ArtifactSourceKind::Repository && source.control_plane_url.is_some()
        }) {
            return Err(ToolError::InvalidParam {
                message: "control_plane_url is supported only for Depot sources".to_owned(),
                param: "control_plane_url".to_owned(),
            });
        }
        let mut clients = BTreeMap::new();
        for source in config.sources.iter().filter(|source| {
            source.kind == ArtifactSourceKind::Depot && source.control_plane_url.is_some()
        }) {
            if clients.contains_key(&source.id) {
                return Err(ToolError::Conflict {
                    message: "Duplicate Artifact authority connection".to_owned(),
                    existing_id: source.id.clone(),
                });
            }
            let control_plane_url = source
                .control_plane_url
                .as_deref()
                .expect("filtered to configured control-plane URLs");
            let parsed = labby_primitives::ssrf::parse_validated_https_url(control_plane_url)
                .map_err(|_| ToolError::InvalidParam {
                    message: "Artifact control-plane URL must be a public HTTPS origin".to_owned(),
                    param: "control_plane_url".to_owned(),
                })?;
            if parsed.path() != "/" {
                return Err(ToolError::InvalidParam {
                    message: "Artifact control-plane URL must not include a path".to_owned(),
                    param: "control_plane_url".to_owned(),
                });
            }
            for address in &source.pinned_addresses {
                labby_primitives::ssrf::check_ip_not_private(*address, "Artifact authority")
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority pin must be a public address".to_owned(),
                        param: "pinned_addresses".to_owned(),
                    })?;
            }
            clients.insert(
                source.id.clone(),
                AuthorityConnection {
                    control_plane_url: control_plane_url.to_owned(),
                    pinned_addresses: source.pinned_addresses.clone(),
                    bearer_token_env: source.bearer_token_env.clone(),
                    permits: Arc::new(tokio::sync::Semaphore::new(16)),
                },
            );
        }
        let delegation = delegation_configuration(depot)?;
        Ok(Self {
            clients,
            delegation,
        })
    }

    /// Whether this process issues managed-Depot delegated assertions.
    pub(crate) fn delegation_configured(&self) -> bool {
        self.delegation.is_some()
    }

    /// Per-request signer for the exact-artifact read routes
    /// (`/api/artifacts/exact`, `/api/artifacts/acquire`,
    /// `/api/artifacts/components/...`). Returns `None` outside managed mode so
    /// standalone Depot keeps using the connection credential alone.
    pub(crate) fn read_assertion_provider(
        &self,
        context: AuthorityContext,
    ) -> Option<Arc<dyn ArtifactRequestHeaderProvider>> {
        let delegation = Arc::clone(self.delegation.as_ref()?);
        Some(Arc::new(ReadAssertionProvider {
            delegation,
            context,
        }))
    }

    /// Managed mutations require a live, acknowledged authority projection,
    /// protocol v1, and the kill switch to be off. Reads are never gated here
    /// so a stale projection stays diagnosable.
    fn require_managed_mutations_ready(&self, operation: Operation) -> Result<(), ToolError> {
        let Some(delegation) = self.delegation.as_ref() else {
            return Ok(());
        };
        if !operation_is_mutation(operation) {
            return Ok(());
        }
        let gate = crate::config::depot::DepotPreferences {
            control_mode: crate::config::depot::DepotControlMode::LabbyManaged,
            managed_authority_kill_switch: delegation.managed_authority_kill_switch,
            ..Default::default()
        };
        if crate::dispatch::depot::authority_projection::managed_mutations_ready(&gate) {
            return Ok(());
        }
        tracing::warn!(
            service = "artifacts",
            operation = operation.provider_name(),
            kill_switch = delegation.managed_authority_kill_switch,
            kind = "service_unavailable",
            "managed Depot mutation refused: authority projection is not ready"
        );
        Err(ToolError::Sdk {
            sdk_kind: "service_unavailable".to_owned(),
            message: "Managed Depot authority is not ready for mutations".to_owned(),
        })
    }

    pub(crate) async fn execute(
        &self,
        connection_id: Option<&str>,
        operation: Operation,
        params: &Value,
        context: Option<&AuthorityContext>,
    ) -> Result<Value, ToolError> {
        let connection = self.connection(connection_id)?;
        let _permit = tokio::time::timeout(Duration::from_secs(2), connection.permits.acquire())
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "queue_saturated".to_owned(),
                message: "Artifact authority request queue is saturated".to_owned(),
            })?
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority connection is unavailable".to_owned(),
            })?;
        self.require_managed_mutations_ready(operation)?;
        let refreshed_context = match context {
            Some(context) => Some(context.revalidate().await?),
            None => None,
        };
        let context = refreshed_context.as_ref();
        // The assertion binds the exact body bytes. Canonicalize once (sorted
        // keys, compact, integers only) and send that same `Value`. The
        // labby-apis client has no raw-byte execution path; it serializes the
        // `Value` with `serde_json::to_vec`, the same encoder used for
        // `body` below. Canonical serialization is deterministic (a
        // `Map<String, Value>` with sorted keys, no floats, fixed escaping), so
        // the bytes on the wire are byte-identical to the digested buffer.
        // `canonical_body_bytes_match_wire_serialization` pins this.
        let params = canonical_json(params)?;
        let body = serde_json::to_vec(&params).map_err(|_| ToolError::InvalidParam {
            message: "Control-plane parameters are not serializable".to_owned(),
            param: "params".to_owned(),
        })?;
        let client = connection.client(context)?;
        let headers =
            self.delegation_headers(operation, &params, context, Some(BodyBinding::of(&body)))?;
        let result = client
            .execute_with_headers(operation, &params, headers)
            .await
            .map_err(map_api_error)?;
        Ok(redact_provider_metadata(result))
    }

    pub(crate) async fn upload(
        &self,
        connection_id: Option<&str>,
        upload_id: &str,
        body: reqwest::Body,
        content_length: Option<u64>,
        content_type: &str,
        content_digest: &str,
        context: &AuthorityContext,
    ) -> Result<Value, ToolError> {
        let connection = self.connection(connection_id)?;
        let _permit = tokio::time::timeout(Duration::from_secs(2), connection.permits.acquire())
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "queue_saturated".to_owned(),
                message: "Artifact authority request queue is saturated".to_owned(),
            })?
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority connection is unavailable".to_owned(),
            })?;
        self.require_managed_mutations_ready(Operation::UploadsCreate)?;
        // The per-connection admission wait above is the final async boundary
        // before assertion minting and the outbound mutation. Reauthorize here
        // so a role or membership revocation cannot age across that wait.
        let context = context.revalidate().await?;
        let client = connection.client(Some(&context))?;
        let headers =
            self.upload_delegation_headers(upload_id, content_digest, content_length, &context)?;
        let result = client
            .upload_with_headers(upload_id, body, content_length, content_type, headers)
            .await
            .map_err(map_api_error)?;
        Ok(redact_provider_metadata(result))
    }

    fn connection(&self, connection_id: Option<&str>) -> Result<&AuthorityConnection, ToolError> {
        match connection_id {
            Some(id) => self.clients.get(id),
            None if self.clients.len() == 1 => self.clients.values().next(),
            None => None,
        }
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: if connection_id.is_none() && self.clients.len() > 1 {
                "Multiple Artifact authorities are configured; connection_id is required".to_owned()
            } else {
                "Requested Artifact authority is not configured".to_owned()
            },
        })
    }

    pub(crate) fn connections(&self) -> Value {
        let connections = self
            .clients
            .keys()
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>();
        serde_json::json!({
            "connections": connections,
            "default_connection_id": (self.clients.len() == 1)
                .then(|| self.clients.keys().next().cloned())
                .flatten(),
        })
    }

    fn delegation_headers(
        &self,
        operation: Operation,
        params: &Value,
        context: Option<&AuthorityContext>,
        body: Option<BodyBinding>,
    ) -> Result<reqwest::header::HeaderMap, ToolError> {
        let Some(delegation) = self.delegation.as_ref() else {
            return Ok(reqwest::header::HeaderMap::new());
        };
        let context = context.ok_or_else(delegation_unavailable)?;
        let intent_id = operation_intent_id(operation, params, context)?;
        let resource = format!("/api/operations/{}", operation.provider_name());
        let assertion = delegation.issue(
            context,
            "POST",
            resource,
            operation.provider_name().to_owned(),
            intent_id.clone(),
            body,
            operation_scopes(operation, context.permission)?,
            operation_capabilities(operation, context)?,
        )?;
        delegation_header_map(context, &assertion, &intent_id)
    }

    fn upload_delegation_headers(
        &self,
        upload_id: &str,
        content_digest: &str,
        content_length: Option<u64>,
        context: &AuthorityContext,
    ) -> Result<reqwest::header::HeaderMap, ToolError> {
        let Some(delegation) = self.delegation.as_ref() else {
            return Ok(reqwest::header::HeaderMap::new());
        };
        let intent_id = uuid::Uuid::new_v4().simple().to_string();
        let resource = format!("/uploads/{}", HttpClient::encode_path_segment(upload_id));
        let content_length = content_length.ok_or_else(|| ToolError::InvalidParam {
            message: "Artifact upload length is required for a delegated upload".to_owned(),
            param: "content-length".to_owned(),
        })?;
        let assertion = delegation.issue(
            context,
            "PUT",
            resource,
            "depot.uploads.put".to_owned(),
            intent_id.clone(),
            Some(BodyBinding {
                content_digest: content_digest.to_owned(),
                content_length,
            }),
            operation_scopes(Operation::UploadsCreate, context.permission)?,
            operation_capabilities(Operation::UploadsCreate, context)?,
        )?;
        delegation_header_map(context, &assertion, &intent_id)
    }
}

impl DelegationConfiguration {
    /// Issue one short-lived assertion. Every field the verifier binds
    /// (method, resource, operation, intent, body binding, scopes,
    /// capabilities, epochs) is supplied by the caller; nothing is inferred.
    #[allow(clippy::too_many_arguments)]
    fn issue(
        &self,
        context: &AuthorityContext,
        method: &str,
        resource: String,
        operation: String,
        intent_id: String,
        body: Option<BodyBinding>,
        scopes: &'static [&'static str],
        capabilities: Vec<String>,
    ) -> Result<String, ToolError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| delegation_unavailable())?
            .as_secs();
        let (content_digest, content_length) = match body {
            Some(body) => (Some(body.content_digest), Some(body.content_length)),
            None => (None, None),
        };
        let claims = DepotDelegationClaims {
            iss: ASSERTION_ISSUER.into(),
            sub: context.actor_id.clone(),
            aud: ASSERTION_AUDIENCE.into(),
            iat: now,
            nbf: now,
            exp: now + 30,
            jti: uuid::Uuid::new_v4().simple().to_string(),
            deployment_id: self.deployment_id.clone(),
            account_id: context.organization_id.clone(),
            organization_id: context.organization_id.clone(),
            team_id: context.team_id.clone(),
            project_id: Some(context.project_id.clone()),
            principal_id: context.actor_id.clone(),
            method: method.into(),
            resource,
            operation,
            intent_id,
            content_digest,
            content_length,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            capabilities,
            epochs: context.epochs.clone(),
            delegation_chain: vec!["labby".into()],
        };
        self.signer
            .issue(claims)
            .map_err(|_| delegation_unavailable())
    }
}

/// Signs delegated read assertions for the exact-artifact routes. The
/// runtime reports the method and request path; the operation name is the
/// one Depot's read context checks for that route, `intent_id` is a fresh
/// `idempotency-key`, and no content claims are made.
struct ReadAssertionProvider {
    delegation: Arc<DelegationConfiguration>,
    context: AuthorityContext,
}

impl ArtifactRequestHeaderProvider for ReadAssertionProvider {
    fn headers(
        &self,
        method: &reqwest::Method,
        path: &str,
    ) -> Result<reqwest::header::HeaderMap, labby_runtime::artifacts::ArtifactError> {
        let unavailable =
            || labby_runtime::artifacts::ArtifactError::Conflict("provider_delegation_unavailable");
        // The runtime classifies the route; unknown paths fail closed there
        // and here rather than guessing an operation.
        let operation = labby_runtime::artifacts::provider::acquisition_operation_for_path(path)
            .ok_or_else(unavailable)?;
        let intent_id = uuid::Uuid::new_v4().simple().to_string();
        let assertion = self
            .delegation
            .issue(
                &self.context,
                method.as_str(),
                path.to_owned(),
                operation.to_owned(),
                intent_id.clone(),
                None,
                &["skills:read"],
                read_capabilities(&self.context),
            )
            .map_err(|_| unavailable())?;
        delegation_header_map(&self.context, &assertion, &intent_id).map_err(|_| unavailable())
    }
}

/// Recursively sort object keys so the signed body is canonical JSON: compact,
/// key-sorted, integers only. Floats never reach the signer.
fn canonical_json(value: &Value) -> Result<Value, ToolError> {
    Ok(match value {
        Value::Object(object) => {
            let mut sorted = object
                .iter()
                .map(|(key, value)| Ok((key.clone(), canonical_json(value)?)))
                .collect::<Result<Vec<_>, ToolError>>()?;
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(canonical_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Number(number) if number.is_f64() => {
            return Err(ToolError::InvalidParam {
                message: "Control-plane parameters must not contain floating-point numbers"
                    .to_owned(),
                param: "params".to_owned(),
            });
        }
        other => other.clone(),
    })
}

/// Durable intent key for a delegated mutation. An explicit `idempotencyKey`
/// wins; otherwise the key is derived from the principal, Project, operation,
/// and canonical parameters so a retry of the same request reuses the same
/// intent and Depot returns the recorded result instead of acting twice.
///
/// The key is a pure function of the request, so no durable intent ledger is
/// needed for retries to converge: a caller who changes the parameters
/// between attempts derives a different key and is treated as a new intent,
/// while Depot's idempotency ledger (keyed by this value) protects the first
/// recorded result of each intent.
fn operation_intent_id(
    operation: Operation,
    params: &Value,
    context: &AuthorityContext,
) -> Result<String, ToolError> {
    use sha2::Digest as _;
    if let Some(explicit) = params.get("idempotencyKey").and_then(Value::as_str) {
        return Ok(explicit.to_owned());
    }
    let canonical =
        serde_json::to_vec(&canonical_json(params)?).map_err(|_| ToolError::InvalidParam {
            message: "Control-plane parameters are not serializable".to_owned(),
            param: "params".to_owned(),
        })?;
    let mut hasher = sha2::Sha256::new();
    for part in [
        context.actor_id.as_bytes(),
        b"\n",
        context.project_id.as_bytes(),
        b"\n",
        operation.provider_name().as_bytes(),
        b"\n",
        &canonical,
    ] {
        hasher.update(part);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn delegation_header_map(
    context: &AuthorityContext,
    assertion: &str,
    intent_id: &str,
) -> Result<reqwest::header::HeaderMap, ToolError> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in [
        ("x-labby-delegation", assertion),
        ("x-labby-organization-id", context.organization_id.as_str()),
        ("x-labby-project-id", context.project_id.as_str()),
        ("idempotency-key", intent_id),
    ] {
        headers.insert(name, value.parse().map_err(|_| delegation_unavailable())?);
    }
    if let Some(team_id) = &context.team_id {
        headers.insert(
            "x-labby-team-id",
            team_id.parse().map_err(|_| delegation_unavailable())?,
        );
    }
    Ok(headers)
}

/// Platform authority is substituted only when it is the sole basis of the
/// local decision: a platform administrator acting without a Team context has
/// no Team-derived capability Depot could evaluate. With a Team selected the
/// exact scoped capability is sent instead.
fn platform_substitution(context: &AuthorityContext) -> bool {
    context.platform_administrator && context.team_id.is_none()
}

fn read_capabilities(context: &AuthorityContext) -> Vec<String> {
    if platform_substitution(context) {
        vec!["platform.manage".into()]
    } else {
        vec!["scope.use".into()]
    }
}

fn operation_capabilities(
    operation: Operation,
    context: &AuthorityContext,
) -> Result<Vec<String>, ToolError> {
    let (capability, required_permission) = operation_authority(operation);
    if context.permission != required_permission {
        return Err(ToolError::Forbidden {
            message: "Remote Artifact operation exceeds its authorized permission".to_owned(),
            required_scopes: Vec::new(),
        });
    }
    if platform_substitution(context) {
        return Ok(vec!["platform.manage".into()]);
    }
    Ok(vec![capability.into()])
}

/// Exact capability and local permission for every curated operation. The
/// match is exhaustive on purpose: a new operation must be classified here
/// before it can be delegated, and deletes never mint `scope.manage`.
fn operation_authority(operation: Operation) -> (&'static str, crate::access::Permission) {
    use crate::access::Permission;
    match operation {
        Operation::ArtifactsList
        | Operation::ArtifactsSearch
        | Operation::SearchSkillsSh
        | Operation::SearchArd
        | Operation::SearchMarketplace
        | Operation::McpRegistryList
        | Operation::AcpRegistryList
        | Operation::AuthorityStatus
        | Operation::SourcesList
        | Operation::BundlesList => ("scope.read", Permission::AssetDiscover),
        Operation::CandidatesList
        | Operation::JobsList
        | Operation::JobsGet
        | Operation::UploadsGet => ("scope.read", Permission::ProjectManage),
        Operation::ArtifactsGet | Operation::BundlesGet => ("scope.use", Permissi