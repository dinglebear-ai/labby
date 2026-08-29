//! Host-neutral OAuth wire vocabulary shared by the authorization server and
//! desktop clients. Transport, browser, storage, and secret ownership remain in
//! their host crates.

use serde::{Deserialize, Serialize};

/// Media type for Labby's versioned server-hosted native authorization start.
pub const NATIVE_AUTHORIZATION_START_MEDIA_TYPE: &str =
    "application/vnd.labby.native-oauth-start+json";

/// Client-facing subset of RFC 8414 metadata plus Labby's native extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientAuthorizationServerMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub native_callback_endpoint: Option<String>,
    #[serde(default)]
    pub native_poll_endpoint_v2: Option<String>,
    #[serde(default)]
    pub native_authorization_start_media_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRegistrationRequest {
    pub redirect_uris: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_id: String,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default = "default_token_endpoint_auth_method")]
    pub token_endpoint_auth_method: String,
}

fn default_token_endpoint_auth_method() -> String {
    "none".to_string()
}

/// Secret-bearing success response. This type intentionally does not implement
/// `Debug`; host clients must wrap its strings in their redacting secret type.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSuccessResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub scope: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthErrorResponse {
    pub error: String,
    pub error_description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAuthorizationStartResponse {
    pub authorization_url: String,
    pub poll_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePollRequest {
    pub poll_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePollResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Accept HTTPS endpoints and RFC 8252 loopback HTTP endpoints only.
pub fn require_secure_endpoint(raw: &str) -> Result<url::Url, String> {
    let endpoint =
        url::Url::parse(raw).map_err(|err| format!("invalid OAuth URL `{raw}`: {err}"))?;
    match endpoint.scheme() {
        "https" => Ok(endpoint),
        "http"
            if matches!(
                endpoint.host_str(),
                Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
            ) =>
        {
            Ok(endpoint)
        }
        _ => Err("OAuth endpoints must use HTTPS (HTTP is allowed only on loopback)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_fixture_accepts_optional_provider_capabilities() {
        let parsed: ClientAuthorizationServerMetadata = serde_json::from_value(serde_json::json!({
            "authorization_endpoint": "https://lab.example/authorize",
            "token_endpoint": "https://lab.example/token",
            "unrelated_rfc_field": true
        }))
        .expect("minimal provider metadata");
        assert!(parsed.registration_endpoint.is_none());
        assert!(parsed.revocation_endpoint.is_none());
    }

    #[test]
    fn token_fixture_accepts_optional_refresh_token() {
        let without: TokenSuccessResponse = serde_json::from_value(serde_json::json!({
            "access_token": "access", "token_type": "Bearer", "expires_in": 3600, "scope": "lab"
        }))
        .unwrap();
        assert!(without.refresh_token.is_none());
        let with: TokenSuccessResponse = serde_json::from_value(serde_json::json!({
            "access_token": "access", "token_type": "Bearer", "expires_in": 3600,
            "refresh_token": "refresh", "scope": "lab"
        }))
        .unwrap();
        assert_eq!(with.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn endpoint_policy_rejects_non_loopback_cleartext() {
        assert!(require_secure_endpoint("https://lab.example/token").is_ok());
        assert!(require_secure_endpoint("http://127.0.0.1:8080/token").is_ok());
        assert!(require_secure_endpoint("http://lab.example/token").is_err());
    }

    #[test]
    fn error_fixture_preserves_standard_fields_and_optional_uri() {
        let error: OAuthErrorResponse = serde_json::from_value(serde_json::json!({
            "error": "invalid_grant",
            "error_description": "authorization grant is invalid"
        }))
        .unwrap();
        assert_eq!(error.error, "invalid_grant");
        assert!(error.error_uri.is_none());
    }
}
