//! Runtime-owned, non-authorizing integration discovery snapshot.

use serde::Serialize;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IntegrationIdentity {
    pub contract_version: &'static str,
    pub product: &'static str,
    pub server_id: String,
    pub product_version: &'static str,
    pub api_version: ApiVersion,
    /// Mounted registered service names, not caller permissions or upstream health.
    pub capabilities: Vec<String>,
    pub auth: IntegrationAuth,
    pub streams: IntegrationStreams,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ApiVersion {
    pub major: u8,
    pub minor: u8,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IntegrationAuth {
    pub modes: Vec<&'static str>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub token_endpoint_origin: Option<String>,
    /// No principal cache authority is published by this discovery contract.
    pub principal_cache_scope: Option<String>,
    /// Never a hash of credentials, nor a fabricated rotation generation.
    pub credential_generation: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IntegrationStreams {
    pub transport: &'static str,
    pub resume: &'static str,
}

impl IntegrationIdentity {
    pub(crate) fn snapshot(
        installation_id: &str,
        static_bearer: bool,
        oauth: Option<&labby_auth::state::AuthState>,
        mut mounted_services: Vec<String>,
    ) -> Self {
        mounted_services.sort();
        mounted_services.dedup();
        let mut modes = Vec::new();
        if static_bearer {
            modes.push("static_bearer");
        }
        if oauth.is_some() {
            modes.push("oauth2");
        }
        let issuer = oauth
            .and_then(|state| state.config.public_url.as_ref())
            .map(|url| url.as_str().trim_end_matches('/').to_string());
        let audience = oauth
            .filter(|state| state.config.public_url.is_some())
            .map(labby_auth::metadata::canonical_resource_url);
        let token_endpoint_origin = oauth
            .and_then(|state| state.config.public_url.as_ref())
            .map(|url| url.origin().ascii_serialization());
        Self {
            contract_version: "1.0.0",
            product: "labby",
            server_id: format!(
                "labby_{}",
                hex::encode(Sha256::digest(installation_id.as_bytes()))
            ),
            product_version: env!("CARGO_PKG_VERSION"),
            api_version: ApiVersion { major: 1, minor: 0 },
            capabilities: mounted_services,
            auth: IntegrationAuth {
                modes,
                issuer,
                audience,
                token_endpoint_origin,
                principal_cache_scope: None,
                credential_generation: None,
            },
            streams: IntegrationStreams {
                transport: "none",
                resume: "none",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn full_schema_rejects_nested_drift_and_cache_authority() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../docs/contracts/integration-identity-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let identity = IntegrationIdentity::snapshot(
            "installation-one",
            true,
            None,
            vec!["setup".into(), "doctor".into(), "setup".into()],
        );
        let value = serde_json::to_value(&identity).unwrap();
        assert!(validator.is_valid(&value));
        assert_eq!(value["capabilities"], json!(["doctor", "setup"]));
        assert!(value["auth"]["issuer"].is_null());
        assert!(value["auth"]["audience"].is_null());
        assert!(value["auth"]["token_endpoint_origin"].is_null());
        for (pointer, invalid) in [
            ("/auth/credential_generation", json!("redacted")),
            ("/auth/principal_cache_scope", json!("issuer-subject")),
            ("/auth/audience", json!(42)),
            ("/auth/modes", json!(["service_identity"])),
            ("/product_version", json!("1x2x3")),
            ("/streams/resume", json!("cursor")),
        ] {
            let mut changed = value.clone();
            *changed.pointer_mut(pointer).unwrap() = invalid;
            assert!(!validator.is_valid(&changed), "accepted {pointer}");
        }
        assert_eq!(
            identity.server_id,
            IntegrationIdentity::snapshot("installation-one", false, None, vec![]).server_id
        );
        assert_ne!(
            identity.server_id,
            IntegrationIdentity::snapshot("installation-two", true, None, vec![]).server_id
        );
    }
}
