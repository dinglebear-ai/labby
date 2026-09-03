//! Server-held remote Artifact authority used by the public control-plane services.

use std::collections::BTreeMap;

use labby_apis::artifact_control::{ArtifactControlClient, Operation};
use labby_apis::core::{ApiError, Auth, HttpClient};
use serde_json::Value;

use crate::config::{ArtifactPreferences, ArtifactSourceKind};
use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactControlPlane {
    clients: BTreeMap<String, ArtifactControlClient>,
}

impl ArtifactControlPlane {
    pub(crate) fn from_config(config: &ArtifactPreferences) -> Result<Self, ToolError> {
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
            let token = source
                .bearer_token_env
                .as_ref()
                .map(|name| std::env::var(name))
                .transpose()
                .map_err(|_| ToolError::Sdk {
                    sdk_kind: "source_unavailable".to_owned(),
                    message: "Artifact authority credential is unavailable".to_owned(),
                })?;
            let auth = token.map_or(Auth::None, |token| Auth::Bearer { token });
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
            let http = HttpClient::with_pinned_addresses(
                control_plane_url,
                auth,
                source.pinned_addresses.iter().copied(),
            )
            .map_err(map_api_error)?;
            clients.insert(source.id.clone(), ArtifactControlClient::new(http));
        }
        Ok(Self { clients })
    }

    pub(crate) async fn execute(
        &self,
        connection_id: Option<&str>,
        operation: Operation,
        params: &Value,
    ) -> Result<Value, ToolError> {
        let client = self.client(connection_id)?;
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
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<Value, ToolError> {
        let client = self.client(connection_id)?;
        let result = client
            .upload(upload_id, bytes, content_type)
            .await
            .map_err(map_api_error)?;
        Ok(redact_provider_metadata(result))
    }

    fn client(&self, connection_id: Option<&str>) -> Result<&ArtifactControlClient, ToolError> {
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
                    let sensitive = normalized.contains("authorization")
                        || normalized.contains("credential")
                        || normalized.contains("secret")
                        || normalized.contains("operator")
                        || normalized.contains("internal")
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
    }
}
