//! Server-held remote Artifact authority used by the public control-plane services.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use labby_apis::artifact_control::{ArtifactControlClient, Operation};
use labby_apis::core::{ApiError, Auth, HttpClient};
use labby_auth::VerifiedIdentity;
use labby_primitives::action::{ActionSpec, ParamSpec};
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

#[derive(Debug, Clone)]
pub(crate) struct AuthorityContext {
    pub actor_id: String,
    pub project_id: String,
}

pub(crate) async fn authorize_authority_context(
    runtime: &crate::access::AccessRuntime,
    identity: VerifiedIdentity,
    project_id: &str,
    permission: crate::access::Permission,
) -> Result<AuthorityContext, ToolError> {
    let store = runtime.store().await.map_err(|_| ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Artifact authorization is unavailable".to_owned(),
    })?;
    let snapshot = store
        .authorize_skill_library(identity, project_id.to_owned(), permission)
        .await
        .map_err(|_| ToolError::Forbidden {
            message: "Remote Artifact operation is not authorized for this project".to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        })?;
    Ok(AuthorityContext {
        actor_id: snapshot.principal_id,
        project_id: snapshot.project_id,
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactControlPlane {
    clients: BTreeMap<String, AuthorityConnection>,
}

#[derive(Debug, Clone)]
struct AuthorityConnection {
    control_plane_url: String,
    pinned_addresses: Vec<IpAddr>,
    bearer_token_env: Option<String>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl ArtifactControlPlane {
    pub(crate) fn from_config(config: &ArtifactPreferences) -> Result<Self, ToolError> {
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
        Ok(Self { clients })
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
        let client = connection.client(context)?;
        let result = client
            .execute(operation, params)
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
        let client = connection.client(Some(context))?;
        let result = client
            .upload(upload_id, body, content_length, content_type)
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

    use serde_json::json;

    use super::{ArtifactControlPlane, redact_provider_metadata};
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
}
