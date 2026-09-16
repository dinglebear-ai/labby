use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::dispatch::error::ToolError;

const PROFILE_SCHEMA_VERSION: &str = "labby.organization-bootstrap/v1";
const SIGNING_DOMAIN: &[u8] = b"labby.organization-bootstrap.v1\0";
const LINEAR_NAME: &str = "linear";
const LINEAR_URL: &str = "https://notification-worker.unraid-workers.workers.dev/mcp/linear";
const TEAM_DEPOT_NAME: &str = "team-depot";
pub(crate) const TEAM_DEPOT_URL_ENV: &str = "LABBY_ORGANIZATION_BOOTSTRAP_TEAM_DEPOT_URL";
pub(crate) const SIGNING_KEY_ID_ENV: &str = "LABBY_ORGANIZATION_BOOTSTRAP_KEY_ID";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OrganizationBootstrapProfile {
    pub schema_version: String,
    pub organization_id: String,
    pub issued_at: i64,
    pub key_id: String,
    pub verifying_key: String,
    pub integrations: Vec<BootstrapIntegration>,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BootstrapIntegration {
    pub name: String,
    pub display_name: String,
    pub url: String,
    pub proxy_resources: bool,
    pub proxy_prompts: bool,
    pub proxy_skills: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<BootstrapOauth>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BootstrapOauth {
    pub mode: String,
    pub registration_strategy: String,
}

#[derive(Serialize)]
struct UnsignedProfile<'a> {
    schema_version: &'a str,
    organization_id: &'a str,
    issued_at: i64,
    key_id: &'a str,
    verifying_key: &'a str,
    integrations: &'a [BootstrapIntegration],
}

pub(crate) fn create(params: &Value) -> Result<Value, ToolError> {
    let organization_id = required_string(params, "organization_id")?;
    let key_id = required_string(params, "key_id")?;
    let team_depot_url = required_https_url(params, "team_depot_url")?;
    let signing_env = params
        .get("signing_key_env")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(crate::config::depot::DEFAULT_AUTHORITY_SIGNING_KEY_ENV);
    if !crate::config::depot::allowed_secret_reference(signing_env) {
        return Err(invalid(
            "signing_key_env",
            "invalid signing key environment reference",
        ));
    }
    let encoded = Zeroizing::new(std::env::var(signing_env).map_err(|_| ToolError::Sdk {
        sdk_kind: "configuration".into(),
        message: format!("organization profile signing key environment {signing_env} is not set"),
    })?);
    let decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded.trim()).map_err(|_| {
        invalid(
            "signing_key_env",
            "signing key must be base64url without padding",
        )
    })?);
    let secret: [u8; 32] = decoded
        .as_slice()
        .try_into()
        .map_err(|_| invalid("signing_key_env", "signing key must decode to 32 bytes"))?;
    let signing_key = SigningKey::from_bytes(&secret);
    let verifying_key = signing_key.verifying_key();
    let verifying_key_encoded = URL_SAFE_NO_PAD.encode(verifying_key.to_bytes());
    let issued_at = unix_now()?;
    let integrations = vec![
        BootstrapIntegration {
            name: TEAM_DEPOT_NAME.into(),
            display_name: "Team Depot".into(),
            url: team_depot_url,
            proxy_resources: true,
            proxy_prompts: true,
            proxy_skills: true,
            oauth: None,
        },
        BootstrapIntegration {
            name: LINEAR_NAME.into(),
            display_name: "Linear Notifications".into(),
            url: LINEAR_URL.into(),
            proxy_resources: true,
            proxy_prompts: true,
            proxy_skills: false,
            oauth: Some(BootstrapOauth {
                mode: "authorization_code_pkce".into(),
                registration_strategy: "dynamic".into(),
            }),
        },
    ];
    let unsigned = UnsignedProfile {
        schema_version: PROFILE_SCHEMA_VERSION,
        organization_id: &organization_id,
        issued_at,
        key_id: &key_id,
        verifying_key: &verifying_key_encoded,
        integrations: &integrations,
    };
    let signing_input = signing_input(&unsigned)?;
    let signature = URL_SAFE_NO_PAD.encode(signing_key.sign(&signing_input).to_bytes());
    let profile = OrganizationBootstrapProfile {
        schema_version: PROFILE_SCHEMA_VERSION.into(),
        organization_id,
        issued_at,
        key_id,
        verifying_key: verifying_key_encoded,
        integrations,
        signature,
    };
    let signer_fingerprint = signer_fingerprint(&verifying_key);
    Ok(json!({"profile": profile, "signer_fingerprint": signer_fingerprint}))
}

pub(crate) fn configured_offer(organization_id: &str) -> Result<Option<Value>, ToolError> {
    let Some(team_depot_url) = std::env::var(TEAM_DEPOT_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(key_id) = std::env::var(SIGNING_KEY_ID_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    create(&json!({
        "organization_id": organization_id,
        "team_depot_url": team_depot_url,
        "key_id": key_id
    }))
    .map(Some)
}

pub(crate) fn preview(params: &Value) -> Result<Value, ToolError> {
    let profile = parse_profile(params)?;
    let signer_fingerprint = verify_profile(&profile)?;
    Ok(json!({
        "valid": true,
        "schema_version": profile.schema_version,
        "organization_id": profile.organization_id,
        "issued_at": profile.issued_at,
        "key_id": profile.key_id,
        "signer_fingerprint": signer_fingerprint,
        "integrations": profile.integrations,
        "mutates_personal_runtime": false,
        "runtime_authority": "personal_labby"
    }))
}

#[cfg(feature = "gateway")]
pub(crate) async fn apply(params: &Value) -> Result<Value, ToolError> {
    let profile = parse_profile(params)?;
    let actual_fingerprint = verify_profile(&profile)?;
    let expected_fingerprint = required_string(params, "expected_signer_fingerprint")?;
    if !constant_time_eq(&actual_fingerprint, expected_fingerprint.trim()) {
        return Err(ToolError::Forbidden {
            message: "organization profile signer does not match the expected trust anchor".into(),
            required_scopes: Vec::new(),
        });
    }
    let specs = profile
        .integrations
        .iter()
        .map(integration_to_upstream)
        .collect::<Result<Vec<_>, _>>()?;
    let manager =
        crate::dispatch::gateway::current_gateway_manager().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "gateway manager is unavailable".into(),
        })?;

    // Organization defaults are additive. Preserve an existing personal
    // configuration when it already owns the same canonical name + endpoint,
    // but fail before mutation if the name points somewhere else. This keeps
    // personal configuration authoritative and avoids partial bootstrap state.
    let mut missing = Vec::new();
    let mut preserved = Vec::new();
    for spec in specs {
        match manager.get(&spec.name).await {
            Ok(existing) => {
                if existing.config.url == spec.url
                    && existing.config.command.is_none()
                    && spec.command.is_none()
                {
                    preserved.push(spec.name);
                } else {
                    return Err(ToolError::Conflict {
                        message: format!(
                            "organization profile upstream {} conflicts with an existing personal gateway definition",
                            spec.name
                        ),
                        existing_id: spec.name,
                    });
                }
            }
            Err(ToolError::Sdk { sdk_kind, .. }) if sdk_kind == "not_found" => missing.push(spec),
            Err(error) => return Err(error),
        }
    }

    let applied = manager
        .batch_add_atomic(missing, Some("setup.organization_profile.apply"), None)
        .await?;
    Ok(json!({
        "applied": applied,
        "preserved": preserved,
        "organization_id": profile.organization_id,
        "signer_fingerprint": actual_fingerprint,
        "runtime_authority": "personal_labby"
    }))
}

#[cfg(not(feature = "gateway"))]
pub(crate) async fn apply(_params: &Value) -> Result<Value, ToolError> {
    Err(ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "organization profile apply requires gateway support".into(),
    })
}

fn integration_to_upstream(
    integration: &BootstrapIntegration,
) -> Result<labby_runtime::gateway_config::UpstreamConfig, ToolError> {
    validate_integration(integration)?;
    let mut value = json!({
        "name": integration.name,
        "display_name": integration.display_name,
        "enabled": true,
        "url": integration.url,
        "transport": "http",
        "proxy_resources": integration.proxy_resources,
        "proxy_prompts": integration.proxy_prompts,
        "proxy_skills": integration.proxy_skills
    });
    if let Some(oauth) = &integration.oauth {
        value["oauth"] = json!({
            "mode": oauth.mode,
            "registration": {"strategy": oauth.registration_strategy},
            "prefer_client_metadata_document": false
        });
    }
    serde_json::from_value(value)
        .map_err(|error| invalid("profile", &format!("invalid upstream projection: {error}")))
}

fn parse_profile(params: &Value) -> Result<OrganizationBootstrapProfile, ToolError> {
    let value = params
        .get("profile")
        .cloned()
        .ok_or_else(|| ToolError::MissingParam {
            message: "missing required parameter profile".into(),
            param: "profile".into(),
        })?;
    serde_json::from_value(value)
        .map_err(|error| invalid("profile", &format!("invalid organization profile: {error}")))
}

fn verify_profile(profile: &OrganizationBootstrapProfile) -> Result<String, ToolError> {
    if profile.schema_version != PROFILE_SCHEMA_VERSION
        || profile.organization_id.trim().is_empty()
        || profile.key_id.trim().is_empty()
        || profile.integrations.len() != 2
    {
        return Err(invalid(
            "profile",
            "unsupported or malformed organization profile",
        ));
    }
    for integration in &profile.integrations {
        validate_integration(integration)?;
    }
    if profile.integrations[0].name != TEAM_DEPOT_NAME
        || profile.integrations[1].name != LINEAR_NAME
        || profile.integrations[1].url != LINEAR_URL
    {
        return Err(invalid(
            "profile",
            "organization profile integrations are not canonical",
        ));
    }
    let key_bytes = URL_SAFE_NO_PAD
        .decode(profile.verifying_key.as_bytes())
        .map_err(|_| invalid("profile", "invalid verifying key encoding"))?;
    let key_bytes: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| invalid("profile", "invalid verifying key length"))?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| invalid("profile", "invalid verifying key"))?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(profile.signature.as_bytes())
        .map_err(|_| invalid("profile", "invalid signature encoding"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| invalid("profile", "invalid signature"))?;
    let unsigned = UnsignedProfile {
        schema_version: &profile.schema_version,
        organization_id: &profile.organization_id,
        issued_at: profile.issued_at,
        key_id: &profile.key_id,
        verifying_key: &profile.verifying_key,
        integrations: &profile.integrations,
    };
    verifying_key
        .verify(&signing_input(&unsigned)?, &signature)
        .map_err(|_| {
            invalid(
                "profile",
                "organization profile signature verification failed",
            )
        })?;
    Ok(signer_fingerprint(&verifying_key))
}

fn validate_integration(integration: &BootstrapIntegration) -> Result<(), ToolError> {
    let url = url::Url::parse(&integration.url)
        .map_err(|_| invalid("profile", "integration URL is invalid"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(invalid(
            "profile",
            "integration URLs must be credential-free HTTPS origins or paths",
        ));
    }
    Ok(())
}

fn required_https_url(params: &Value, name: &str) -> Result<String, ToolError> {
    let value = required_string(params, name)?;
    let integration = BootstrapIntegration {
        name: "validate".into(),
        display_name: "Validate".into(),
        url: value.clone(),
        proxy_resources: false,
        proxy_prompts: false,
        proxy_skills: false,
        oauth: None,
    };
    validate_integration(&integration)?;
    Ok(value)
}

fn signing_input(unsigned: &UnsignedProfile<'_>) -> Result<Vec<u8>, ToolError> {
    let body = serde_json::to_vec(unsigned).map_err(|error| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("failed to serialize organization profile: {error}"),
    })?;
    let mut input = Vec::with_capacity(SIGNING_DOMAIN.len() + body.len());
    input.extend_from_slice(SIGNING_DOMAIN);
    input.extend_from_slice(&body);
    Ok(input)
}

fn signer_fingerprint(key: &VerifyingKey) -> String {
    hex::encode(Sha256::digest(key.to_bytes()))
}

fn required_string(params: &Value, name: &str) -> Result<String, ToolError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ToolError::MissingParam {
            message: format!("missing required parameter {name}"),
            param: name.into(),
        })
}

fn invalid(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: param.into(),
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

fn unix_now() -> Result<i64, ToolError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .map_err(|_| ToolError::internal_message("system clock unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_profile_round_trip_and_tamper_rejection() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let integrations = vec![
            BootstrapIntegration {
                name: TEAM_DEPOT_NAME.into(),
                display_name: "Team Depot".into(),
                url: "https://team.example/mcp".into(),
                proxy_resources: true,
                proxy_prompts: true,
                proxy_skills: true,
                oauth: None,
            },
            BootstrapIntegration {
                name: LINEAR_NAME.into(),
                display_name: "Linear Notifications".into(),
                url: LINEAR_URL.into(),
                proxy_resources: true,
                proxy_prompts: true,
                proxy_skills: false,
                oauth: Some(BootstrapOauth {
                    mode: "authorization_code_pkce".into(),
                    registration_strategy: "dynamic".into(),
                }),
            },
        ];
        let verifying = URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());
        let unsigned = UnsignedProfile {
            schema_version: PROFILE_SCHEMA_VERSION,
            organization_id: "org",
            issued_at: 123,
            key_id: "k1",
            verifying_key: &verifying,
            integrations: &integrations,
        };
        let signature =
            URL_SAFE_NO_PAD.encode(key.sign(&signing_input(&unsigned).unwrap()).to_bytes());
        let mut profile = OrganizationBootstrapProfile {
            schema_version: PROFILE_SCHEMA_VERSION.into(),
            organization_id: "org".into(),
            issued_at: 123,
            key_id: "k1".into(),
            verifying_key: verifying,
            integrations,
            signature,
        };
        assert!(verify_profile(&profile).is_ok());
        profile.integrations[0].url = "https://evil.example/mcp".into();
        assert!(verify_profile(&profile).is_err());
    }

    #[test]
    fn integration_projection_is_non_secret_and_linear_uses_dynamic_oauth() {
        let integration = BootstrapIntegration {
            name: LINEAR_NAME.into(),
            display_name: "Linear Notifications".into(),
            url: LINEAR_URL.into(),
            proxy_resources: true,
            proxy_prompts: true,
            proxy_skills: false,
            oauth: Some(BootstrapOauth {
                mode: "authorization_code_pkce".into(),
                registration_strategy: "dynamic".into(),
            }),
        };
        let config = integration_to_upstream(&integration).unwrap();
        assert!(config.bearer_token_env.is_none());
        assert!(config.env.is_empty());
        assert!(config.oauth.is_some());
    }
}
