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
        Operation::ArtifactsGet | Operation::BundlesGet => ("scope.use", Permission::AssetUse),
        Operation::CandidatesIntake
        | Operation::ArtifactsFork
        | Operation::JobsStart
        | Operation::UploadsCreate
        | Operation::BundlesCreate
        | Operation::BundlesAddArtifact => ("scope.create", Permission::ProjectManage),
        Operation::JobsCancel
        | Operation::JobsRetry
        | Operation::SourcesRefresh
        | Operation::BundlesPublish
        | Operation::BundlesRemoveArtifact => ("scope.operate", Permission::ProjectManage),
        Operation::ArtifactsFollow
        | Operation::ArtifactsSetPublication
        | Operation::ArtifactsSetLicense
        | Operation::SourcesConfigure
        | Operation::BundlesSetVisibility => ("scope.manage", Permission::ProjectManage),
        Operation::SourcesDelete | Operation::UploadsDelete | Operation::BundlesDelete => {
            ("scope.delete", Permission::ProjectManage)
        }
    }
}

pub(crate) fn operation_permission(operation: Operation) -> crate::access::Permission {
    operation_authority(operation).1
}

/// Reads and exact-use never gate on the managed projection; everything else
/// changes Depot state and must wait for a live acknowledged projection.
fn operation_is_mutation(operation: Operation) -> bool {
    !matches!(operation_authority(operation).0, "scope.read" | "scope.use")
}

/// Transport scopes follow the operation's capability class, never the local
/// permission alone: read-only operations that happen to require
/// `ProjectManage` locally (candidate, job, and upload listings) still carry
/// only `skills:read`.
fn operation_scopes(
    operation: Operation,
    permission: crate::access::Permission,
) -> Result<&'static [&'static str], ToolError> {
    // Validate the permission/capability pair before emitting broader transport
    // scopes. Depot treats both fields as authority, so neither may exceed the
    // exact Labby decision.
    let (capability, required_permission) = operation_authority(operation);
    if permission != required_permission {
        return Err(ToolError::Forbidden {
            message: "Remote Artifact operation exceeds its authorized permission".to_owned(),
            required_scopes: Vec::new(),
        });
    }
    Ok(match capability {
        "scope.read" | "scope.use" => &["skills:read"],
        _ => &["skills:read", "skills:write"],
    })
}

fn delegation_configuration(
    depot: &crate::config::depot::DepotPreferences,
) -> Result<Option<Arc<DelegationConfiguration>>, ToolError> {
    if depot.control_mode != crate::config::depot::DepotControlMode::LabbyManaged {
        return Ok(None);
    }
    let configured = [
        depot.authority_installation_id.as_ref(),
        depot.authority_key_id.as_ref(),
        depot.authority_signing_key_env.as_ref(),
    ];
    if configured.iter().all(|value| value.is_none()) {
        return Err(delegation_unavailable());
    }
    let [Some(deployment_id), Some(key_id), Some(key_env)] = configured else {
        return Err(delegation_unavailable());
    };
    // The seeds are read, decoded, parsed, and zeroized inside labby-auth; a
    // malformed key fails here rather than at the first signature. Overlap
    // keys registered for rotation stay valid for verification while the
    // active key signs new assertions.
    let signer = if depot.authority_overlap_signing_keys.is_empty() {
        DepotDelegationSigner::from_seed_env(key_id.clone(), key_env)
    } else {
        DepotDelegationSigner::from_seed_envs(
            key_id.clone(),
            std::iter::once((key_id.as_str(), key_env.as_str())).chain(
                depot
                    .authority_overlap_signing_keys
                    .iter()
                    .map(|overlap| (overlap.key_id.as_str(), overlap.signing_key_env.as_str())),
            ),
        )
    }
    .map_err(|_| delegation_unavailable())?;
    Ok(Some(Arc::new(DelegationConfiguration {
        signer,
        deployment_id: deployment_id.clone(),
        managed_authority_kill_switch: depot.managed_authority_kill_switch,
    })))
}

/// Test-only constructor that goes through the same zeroizing, fail-fast
/// labby-auth seed path as `from_seed_env`, minus the environment read (the
/// crate forbids `unsafe`, so tests cannot set process environment).
#[cfg(test)]
fn delegation_configuration_from_seed(
    deployment_id: &str,
    key_id: &str,
    seed: [u8; 32],
) -> Result<Arc<DelegationConfiguration>, ToolError> {
    let signer = DepotDelegationSigner::from_seed(key_id, zeroize::Zeroizing::new(seed))
        .map_err(|_| delegation_unavailable())?;
    Ok(Arc::new(DelegationConfiguration {
        signer,
        deployment_id: deployment_id.to_owned(),
        managed_authority_kill_switch: false,
    }))
}

fn delegation_unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Managed Depot delegation authority is unavailable".to_owned(),
    }
}

impl AuthorityConnection {
    fn client(
        &self,
        context: Option<&AuthorityContext>,
    ) -> Result<ArtifactControlClient, ToolError> {
        let token = self
            .bearer_token_env
            .as_ref()
            .map(|name| std::env::var(name))
            .transpose()
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "source_unavailable".to_owned(),
                message: "Artifact authority credential is unavailable".to_owned(),
            })?;
        let auth = token.map_or(Auth::None, |token| Auth::Bearer { token });
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(context) = context {
            headers.insert(
                "x-labby-actor-id",
                context
                    .actor_id
                    .parse()
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority actor identity is invalid".to_owned(),
                        param: "actor_id".to_owned(),
                    })?,
            );
            headers.insert(
                "x-labby-project-id",
                context
                    .project_id
                    .parse()
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Artifact authority project identity is invalid".to_owned(),
                        param: "project_id".to_owned(),
                    })?,
            );
        }
        HttpClient::with_pinned_addresses_and_headers(
            &self.control_plane_url,
            auth,
            self.pinned_addresses.iter().copied(),
            headers,
        )
        .map(ArtifactControlClient::new)
        .map_err(map_api_error)
    }
}

fn map_api_error(error: ApiError) -> ToolError {
    match error {
        ApiError::Auth => ToolError::Forbidden {
            message: "Artifact authority rejected its server credential".to_owned(),
            required_scopes: Vec::new(),
        },
        ApiError::NotFound => ToolError::Sdk {
            sdk_kind: "not_found".to_owned(),
            message: "Remote Artifact control-plane item was not found".to_owned(),
        },
        ApiError::RateLimited { .. } => ToolError::Sdk {
            sdk_kind: "rate_limited".to_owned(),
            message: "Artifact authority is rate limited; retry later".to_owned(),
        },
        ApiError::Validation { field, .. } => ToolError::InvalidParam {
            message: "Artifact authority rejected a parameter".to_owned(),
            param: field,
        },
        ApiError::Network(_) => ToolError::Sdk {
            sdk_kind: "service_unavailable".to_owned(),
            message: "Artifact authority is unreachable".to_owned(),
        },
        ApiError::Server { status: 409, .. } => ToolError::Conflict {
            message: "Artifact authority state conflicts with this request".to_owned(),
            existing_id: "remote_artifact_state".to_owned(),
        },
        ApiError::Server { .. } => ToolError::Sdk {
            sdk_kind: "service_unavailable".to_owned(),
            message: "Artifact authority operation failed".to_owned(),
        },
        ApiError::Decode(_) | ApiError::Internal(_) => ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Artifact authority returned an invalid response".to_owned(),
        },
    }
}

fn redact_provider_metadata(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .filter_map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    let compact = normalized
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .collect::<String>();
                    let safe_opaque_token =
                        matches!(compact.as_str(), "pagetoken" | "nextpagetoken");
                    let sensitive = normalized.contains("authorization")
                        || normalized.contains("credential")
                        || normalized.contains("secret")
                        || normalized.contains("operator")
                        || normalized.contains("internal")
                        || normalized.contains("password")
                        || normalized.contains("apikey")
                        || normalized.contains("api_key")
                        || normalized.contains("privatekey")
                        || normalized.contains("private_key")
                        || normalized.contains("cookie")
                        || (normalized.contains("token") && !safe_opaque_token)
                        || matches!(
                            compact.as_str(),
                            "token" | "accesstoken" | "bearertoken" | "refreshtoken" | "idtoken"
                        )
                        || normalized == "raw_error"
                        || normalized == "stacktrace";
                    (!sensitive).then(|| (key, redact_provider_metadata(value)))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_provider_metadata).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use base64::Engine as _;
    use serde_json::json;

    use super::{
        ArtifactControlPlane, AuthorityContext, BodyBinding, Operation, canonical_json,
        operation_authority, operation_capabilities, operation_intent_id, operation_is_mutation,
        operation_scopes, redact_provider_metadata,
    };
    use labby_runtime::artifacts::provider::acquisition_operation_for_path;

    fn decode_claims(headers: &reqwest::header::HeaderMap) -> serde_json::Value {
        let token = headers["x-labby-delegation"].to_str().unwrap();
        let payload = token.split('.').nth(1).unwrap();
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .unwrap(),
        )
        .unwrap()
    }

    fn context(team_id: Option<&str>, platform_administrator: bool) -> AuthorityContext {
        AuthorityContext {
            actor_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            team_id: team_id.map(str::to_owned),
            project_id: "project-1".into(),
            platform_administrator,
            permission: crate::access::Permission::ProjectManage,
            epochs: labby_auth::depot_delegation::DelegatedAuthorityEpochs {
                authority_schema: 7,
                organization_policy: 8,
                team_membership: Some(9),
                team_policy: Some(10),
                project_membership: Some(11),
                project_policy: Some(12),
                global_revision: 13,
            },
            revalidation: None,
        }
    }

    fn managed_controls() -> ArtifactControlPlane {
        ArtifactControlPlane {
            clients: Default::default(),
            delegation: Some(
                super::delegation_configuration_from_seed("deployment-1", "current", [7_u8; 32])
                    .unwrap(),
            ),
        }
    }
    use crate::config::{ArtifactPreferences, ArtifactSourceConfig, ArtifactSourceKind};

    #[test]
    fn strips_security_and_operator_fields_but_preserves_product_metadata() {
        let projected = redact_provider_metadata(json!({
            "artifact":{"id":"a", "description":"demo", "licenseEvidence":["MIT"]},
            "credentialRef":"git-main",
            "operatorNotes":"private",
            "nested":{"accessToken":"nope", "pageToken":"continue-opaque", "provenance":{"repository":"repo"}}
        }));
        assert_eq!(projected["artifact"]["id"], "a");
        assert_eq!(projected["artifact"]["licenseEvidence"][0], "MIT");
        assert_eq!(projected["nested"]["provenance"]["repository"], "repo");
        assert_eq!(projected["nested"]["pageToken"], "continue-opaque");
        assert!(projected.get("credentialRef").is_none());
        assert!(projected["nested"].get("accessToken").is_none());
    }

    #[test]
    fn strips_conventional_secret_spellings_and_preserves_page_tokens() {
        let projected = redact_provider_metadata(json!({
            "password": "nope",
            "apiKey": "nope",
            "private_key": "nope",
            "sessionCookie": "nope",
            "githubTokenValue": "nope",
            "pageToken": "safe-page",
            "next_page_token": "safe-next"
        }));
        for key in [
            "password",
            "apiKey",
            "private_key",
            "sessionCookie",
            "githubTokenValue",
        ] {
            assert!(projected.get(key).is_none(), "{key} must be redacted");
        }
        assert_eq!(projected["pageToken"], "safe-page");
        assert_eq!(projected["next_page_token"], "safe-next");
    }

    #[test]
    fn control_plane_origin_and_pins_fail_closed() {
        let source = |url: &str, pin: IpAddr| ArtifactSourceConfig {
            id: "primary".to_owned(),
            kind: ArtifactSourceKind::Depot,
            endpoint: "https://depot.example/v1/exact".to_owned(),
            control_plane_url: Some(url.to_owned()),
            pinned_addresses: vec![pin],
            bearer_token_env: None,
        };
        let with = |source| ArtifactPreferences {
            sources: vec![source],
        };

        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example/api",
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            )))
            .is_err()
        );
        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example",
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            )))
            .is_err()
        );
        assert!(
            ArtifactControlPlane::from_config(&with(source(
                "https://depot.example",
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            )))
            .is_ok()
        );

        let repository = ArtifactSourceConfig {
            id: "repo".to_owned(),
            kind: ArtifactSourceKind::Repository,
            endpoint: "https://repository.example/v1/exact".to_owned(),
            control_plane_url: Some("https://depot.example".to_owned()),
            pinned_addresses: Vec::new(),
            bearer_token_env: None,
        };
        assert!(ArtifactControlPlane::from_config(&with(repository)).is_err());
    }

    #[test]
    fn missing_remote_credential_does_not_prevent_local_startup() {
        let config = ArtifactPreferences {
            sources: vec![ArtifactSourceConfig {
                id: "remote".to_owned(),
                kind: ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/v1/exact".to_owned(),
                control_plane_url: Some("https://depot.example".to_owned()),
                pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
                bearer_token_env: Some("LABBY_TEST_DEFINITELY_MISSING_REMOTE_TOKEN".to_owned()),
            }],
        };
        let controls = ArtifactControlPlane::from_config(&config).unwrap();
        assert_eq!(controls.connections()["connections"][0]["id"], "remote");
        let error = controls.clients["remote"].client(None).unwrap_err();
        assert_eq!(error.kind(), "source_unavailable");
        assert!(!error.to_string().contains("LABBY_TEST"));
    }

    #[test]
    fn connection_discovery_exposes_only_safe_ids() {
        let config = ArtifactPreferences {
            sources: vec![ArtifactSourceConfig {
                id: "primary".to_owned(),
                kind: ArtifactSourceKind::Depot,
                endpoint: "https://depot.example/v1/exact".to_owned(),
                control_plane_url: Some("https://depot.example".to_owned()),
                pinned_addresses: vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))],
                bearer_token_env: Some("PRIVATE_REMOTE_TOKEN".to_owned()),
            }],
        };
        let value = ArtifactControlPlane::from_config(&config)
            .unwrap()
            .connections();
        assert_eq!(value["default_connection_id"], "primary");
        let encoded = value.to_string();
        assert!(!encoded.contains("depot.example"));
        assert!(!encoded.contains("PRIVATE_REMOTE_TOKEN"));
    }

    #[test]
    fn managed_delegation_is_fresh_and_exactly_request_bound() {
        let controls = managed_controls();
        let context = context(Some("team-1"), false);
        let params = json!({"idempotencyKey":"intent-1","kind":"git"});
        let body = serde_json::to_vec(&canonical_json(&params).unwrap()).unwrap();
        let binding = BodyBinding::of(&body);
        let headers = controls
            .delegation_headers(
                Operation::JobsStart,
                &params,
                Some(&context),
                Some(binding.clone()),
            )
            .unwrap();
        assert_eq!(headers["idempotency-key"], "intent-1");
        assert_eq!(headers["x-labby-team-id"], "team-1");
        assert_eq!(headers["x-labby-organization-id"], "organization-1");
        assert_eq!(headers["x-labby-project-id"], "project-1");
        let claims = decode_claims(&headers);
        assert_eq!(claims["method"], "POST");
        assert_eq!(claims["resource"], "/api/operations/depot.ingest.start");
        assert_eq!(claims["operation"], "depot.ingest.start");
        assert_eq!(claims["intent_id"], "intent-1");
        assert_eq!(claims["capabilities"], json!(["scope.create"]));
        assert_eq!(claims["scopes"], json!(["skills:read", "skills:write"]));
        // D4: the assertion binds the exact bytes that go on the wire.
        assert_eq!(claims["content_digest"], binding.content_digest);
        assert_eq!(claims["content_length"], binding.content_length);
        assert!(binding.content_digest.starts_with("sha256:"));
        assert_eq!(binding.content_length, u64::try_from(body.len()).unwrap());
        assert!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap() <= 60);

        let upload_headers = controls
            .upload_delegation_headers(
                "upload-1",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some(12),
                &context,
            )
            .unwrap();
        let upload_claims = decode_claims(&upload_headers);
        assert_eq!(upload_claims["method"], "PUT");
        assert_eq!(
            upload_claims["content_digest"],
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(upload_claims["content_length"], 12);
        assert!(
            controls
                .upload_delegation_headers("upload-1", "sha256:aa", None, &context)
                .is_err(),
            "a delegated upload without a known length cannot be bound"
        );
    }

    #[test]
    fn canonical_json_sorts_keys_and_rejects_floats() {
        let canonical =
            canonical_json(&json!({"z": {"b": 1, "a": [3, {"y": 2, "x": 1}]}, "a": true})).unwrap();
        assert_eq!(
            serde_json::to_string(&canonical).unwrap(),
            r#"{"a":true,"z":{"a":[3,{"x":1,"y":2}],"b":1}}"#
        );
        assert_eq!(
            canonical_json(&json!({"ratio": 0.5})).unwrap_err().kind(),
            "invalid_param"
        );
        // Serializing the canonical value twice yields identical bytes, which
        // is what makes the digest describe the bytes on the wire.
        assert_eq!(
            serde_json::to_vec(&canonical).unwrap(),
            serde_json::to_vec(&canonical).unwrap()
        );
        assert_eq!(
            BodyBinding::of(b"abc"),
            BodyBinding {
                content_digest:
                    "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                content_length: 3,
            }
        );
    }

    /// The delegated assertion digests `serde_json::to_vec(&canonical)`; the
    /// labby-apis client serializes the same `Value` through the same encoder
    /// (`post_json_bounded_with_headers` -> `serde_json`), so the wire bytes
    /// are the digested bytes. Pin the encoder invariants that make this hold:
    /// sorted keys survive a clone, nested arrays keep order, and unicode /
    /// control characters escape identically across two serializations.
    #[test]
    fn canonical_body_bytes_match_wire_serialization() {
        let params =
            json!({"name":"tab\tquote\"snow\u{2603}","n":[1,{"b":2,"a":1}],"x":{"k":null}});
        let canonical = canonical_json(&params).unwrap();
        let digested = serde_json::to_vec(&canonical).unwrap();
        // What the HTTP client would send: an independent serialization of a
        // clone of the same value.
        let wire = serde_json::to_vec(&canonical.clone()).unwrap();
        assert_eq!(digested, wire);
        let binding = BodyBinding::of(&digested);
        assert_eq!(binding, BodyBinding::of(&wire));
        assert_eq!(binding.content_length, u64::try_from(wire.len()).unwrap());
        // Re-canonicalizing the canonical form is a fixed point.
        assert_eq!(
            serde_json::to_vec(&canonical_json(&canonical).unwrap()).unwrap(),
            digested
        );
    }

    #[test]
    fn intent_key_is_deterministic_per_principal_project_operation_and_params() {
        let context_a = context(Some("team-1"), false);
        let first = operation_intent_id(
            Operation::JobsStart,
            &json!({"kind":"git","arguments":{"b":1,"a":2}}),
            &context_a,
        )
        .unwrap();
        let reordered = operation_intent_id(
            Operation::JobsStart,
            &json!({"arguments":{"a":2,"b":1},"kind":"git"}),
            &context_a,
        )
        .unwrap();
        assert_eq!(first, reordered, "canonical params yield the same intent");
        assert_eq!(first.len(), 64);
        let other_params = operation_intent_id(
            Operation::JobsStart,
            &json!({"kind":"git","arguments":{"b":1,"a":3}}),
            &context_a,
        )
        .unwrap();
        assert_ne!(first, other_params);
        let other_operation = operation_intent_id(
            Operation::JobsRetry,
            &json!({"kind":"git","arguments":{"b":1,"a":2}}),
            &context_a,
        )
        .unwrap();
        assert_ne!(first, other_operation);
        let mut context_b = context(Some("team-1"), false);
        context_b.actor_id = "principal-2".into();
        assert_ne!(
            first,
            operation_intent_id(
                Operation::JobsStart,
                &json!({"kind":"git","arguments":{"b":1,"a":2}}),
                &context_b,
            )
            .unwrap()
        );
        assert_eq!(
            operation_intent_id(
                Operation::JobsStart,
                &json!({"idempotencyKey":"explicit"}),
                &context_a
            )
            .unwrap(),
            "explicit"
        );
    }

    #[test]
    fn deletes_mint_scope_delete_and_scopes_follow_the_capability_class() {
        for operation in [
            Operation::SourcesDelete,
            Operation::UploadsDelete,
            Operation::BundlesDelete,
        ] {
            assert_eq!(
                operation_authority(operation).0,
                "scope.delete",
                "{operation:?}"
            );
            assert!(operation_is_mutation(operation));
        }
        for operation in [
            Operation::CandidatesList,
            Operation::JobsList,
            Operation::JobsGet,
            Operation::UploadsGet,
        ] {
            assert_eq!(
                operation_scopes(operation, crate::access::Permission::ProjectManage).unwrap(),
                &["skills:read"],
                "{operation:?} is read-only and must not carry skills:write"
            );
            assert!(!operation_is_mutation(operation));
        }
        assert_eq!(
            operation_scopes(
                Operation::JobsStart,
                crate::access::Permission::ProjectManage
            )
            .unwrap(),
            &["skills:read", "skills:write"]
        );
        assert!(!operation_is_mutation(Operation::ArtifactsGet));
        assert!(operation_is_mutation(Operation::SourcesConfigure));
    }

    #[test]
    fn platform_authority_is_substituted_only_without_a_team_context() {
        let with_team = context(Some("team-1"), true);
        assert_eq!(
            operation_capabilities(Operation::JobsStart, &with_team).unwrap(),
            vec!["scope.create".to_owned()]
        );
        let without_team = context(None, true);
        assert_eq!(
            operation_capabilities(Operation::JobsStart, &without_team).unwrap(),
            vec!["platform.manage".to_owned()]
        );
        let mut member = context(Some("team-1"), false);
        member.permission = crate::access::Permission::AssetDiscover;
        assert_eq!(
            operation_capabilities(Operation::ArtifactsList, &member).unwrap(),
            vec!["scope.read".to_owned()]
        );
    }

    #[test]
    fn managed_mutations_are_gated_on_projection_readiness_and_kill_switch() {
        let controls = managed_controls();
        // No projection has been acknowledged in this process, so managed
        // mutations are refused while reads and exact-use still flow.
        assert!(
            controls
                .require_managed_mutations_ready(Operation::ArtifactsList)
                .is_ok()
        );
        assert!(
            controls
                .require_managed_mutations_ready(Operation::ArtifactsGet)
                .is_ok()
        );
        let error = controls
            .require_managed_mutations_ready(Operation::JobsStart)
            .unwrap_err();
        assert_eq!(error.kind(), "service_unavailable");
        assert!(
            controls
                .require_managed_mutations_ready(Operation::SourcesDelete)
                .is_err()
        );
        let standalone = ArtifactControlPlane::default();
        assert!(
            standalone
                .require_managed_mutations_ready(Operation::JobsStart)
                .is_ok()
        );
        // The kill switch is honored independently of readiness.
        let gate = crate::config::depot::DepotPreferences {
            control_mode: crate::config::depot::DepotControlMode::LabbyManaged,
            managed_authority_kill_switch: true,
            ..Default::default()
        };
        assert!(!gate.managed_mutations_ready(true, 1));
        assert!(!gate.managed_mutations_ready(true, 2));
    }

    #[test]
    fn read_assertions_bind_method_path_and_route_operation_without_content_claims() {
        let controls = managed_controls();
        let provider = controls
            .read_assertion_provider(context(Some("team-1"), false))
            .expect("managed mode issues read assertions");
        let cases = [
            (
                reqwest::Method::POST,
                "/api/artifacts/exact",
                "depot.artifacts.exact",
            ),
            (
                reqwest::Method::POST,
                "/api/artifacts/acquire",
                "depot.artifacts.acquire",
            ),
            (
                reqwest::Method::GET,
                "/api/artifacts/components/abcdef",
                "depot.artifacts.component",
            ),
        ];
        let mut intents = std::collections::BTreeSet::new();
        for (method, path, operation) in cases {
            let headers = provider.headers(&method, path).unwrap();
            let claims = decode_claims(&headers);
            assert_eq!(claims["method"], method.as_str());
            assert_eq!(claims["resource"], path);
            assert_eq!(claims["operation"], operation);
            assert!(claims.get("content_digest").is_none());
            assert!(claims.get("content_length").is_none());
            assert_eq!(claims["scopes"], json!(["skills:read"]));
            assert_eq!(claims["capabilities"], json!(["scope.use"]));
            assert_eq!(headers["x-labby-team-id"], "team-1");
            assert_eq!(
                headers["idempotency-key"].to_str().unwrap(),
                claims["intent_id"].as_str().unwrap()
            );
            intents.insert(claims["intent_id"].as_str().unwrap().to_owned());
        }
        assert_eq!(
            intents.len(),
            3,
            "every read assertion carries a fresh intent"
        );
        assert!(
            provider
                .headers(&reqwest::Method::GET, "/api/operations")
                .is_err(),
            "unknown routes never receive an assertion"
        );
        assert_eq!(
            acquisition_operation_for_path("/api/artifacts/exact"),
            Some("depot.artifacts.exact")
        );
        assert!(
            ArtifactControlPlane::default()
                .read_assertion_provider(context(None, false))
                .is_none()
        );
    }

    #[test]
    fn managed_delegation_configuration_fails_closed_when_incomplete() {
        let depot = crate::config::depot::DepotPreferences {
            control_mode: crate::config::depot::DepotControlMode::LabbyManaged,
            authority_installation_id: Some("deployment-1".into()),
            ..Default::default()
        };
        assert!(
            ArtifactControlPlane::from_configs(&ArtifactPreferences::default(), &depot).is_err()
        );
    }

    #[test]
    fn delegated_capability_cannot_exceed_exact_local_permission() {
        let context = AuthorityContext {
            actor_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            team_id: Some("team-1".into()),
            project_id: "project-1".into(),
            platform_administrator: false,
            permission: crate::access::Permission::AssetDiscover,
            epochs: labby_auth::depot_delegation::DelegatedAuthorityEpochs {
                authority_schema: 1,
                organization_policy: 1,
                team_membership: Some(1),
                team_policy: Some(1),
                project_membership: Some(1),
                project_policy: Some(1),
                global_revision: 1,
            },
            revalidation: None,
        };
        assert!(operation_capabilities(Operation::ArtifactsList, &context).is_ok());
        assert!(operation_capabilities(Operation::JobsStart, &context).is_err());
        assert!(operation_capabilities(Operation::SourcesConfigure, &context).is_err());
    }

    /// Cross-repository system driver. Depot's ExUnit orchestrator supplies a
    /// real production Router endpoint and credentials, then invokes this exact
    /// ignored test. No HTTP contract mock is used here.
    #[tokio::test]
    #[ignore = "run by Depot's managed Labby HTTP orchestrator"]
    async fn real_managed_depot_http_driver() {
        use crate::access::{AssignTeamProjectInput, BootstrapOwnerInput, Permission, ProjectRole};
        use labby_auth::{Authenticator, VerifiedIdentity};

        drop(rustls::crypto::ring::default_provider().install_default());

        let endpoint = std::env::var("LABBY_DEPOT_SYSTEM_ENDPOINT")
            .expect("LABBY_DEPOT_SYSTEM_ENDPOINT is required");
        let bearer_env = "LABBY_DEPOT_SYSTEM_BEARER";
        std::env::var(bearer_env).expect("LABBY_DEPOT_SYSTEM_BEARER is required");
        let seed = [41_u8; 32];
        let directory = tempfile::Builder::new()
            .prefix("labby-depot-system-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = crate::access::AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "managed-system-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(owner.clone(), "System Org", "System Project").unwrap(),
            )
            .await
            .unwrap();
        store
            .assign_team_project(
                AssignTeamProjectInput::new(
                    owner.clone(),
                    "bootstrap-initial-team",
                    "bootstrap-default",
                    ProjectRole::Owner,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let sender = crate::dispatch::depot::authority_projection::AuthorityProjectionSender::new(
            url::Url::parse(&endpoint).unwrap(),
            std::env::var(bearer_env).unwrap(),
            "system-installation",
            "current",
            seed,
            store.clone(),
        )
        .unwrap();
        sender
            .send_current_snapshot(
                "bootstrap-local",
                i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let controls = ArtifactControlPlane {
            clients: [(
                "system".to_owned(),
                super::AuthorityConnection {
                    control_plane_url: endpoint,
                    pinned_addresses: vec!["127.0.0.1".parse().unwrap()],
                    bearer_token_env: Some(bearer_env.into()),
                    permits: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
                },
            )]
            .into_iter()
            .collect(),
            delegation: Some(
                super::delegation_configuration_from_seed("system-installation", "current", seed)
                    .unwrap(),
            ),
        };
        let runtime =
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await;
        let context = super::authorize_authority_context(
            &runtime,
            owner,
            "bootstrap-default",
            None,
            Permission::AssetDiscover,
        )
        .await
        .unwrap();
        let result = controls
            .execute(
                Some("system"),
                Operation::ArtifactsList,
                &json!({"limit":1}),
                Some(&context),
            )
            .await
            .unwrap();
        assert!(result.get("artifacts").is_some());
    }
}
