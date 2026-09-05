use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::AuthError;

pub const GOOGLE_CALLBACK_PATH: &str = "/auth/google/callback";
pub const AUTHELIA_CALLBACK_PATH: &str = "/auth/oidc/callback";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboundProviderKind {
    Google,
    Authelia,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutheliaConfig {
    pub issuer_url: Url,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub trusted_private_origin: Option<TrustedIssuerOrigin>,
    #[serde(default)]
    pub ca_certificate_path: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for AutheliaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutheliaConfig")
            .field("issuer_origin", &origin_label(&self.issuer_url))
            .field("client_id", &self.client_id)
            .field(
                "trusted_private_origin",
                &self.trusted_private_origin.as_ref().map(|_| "<configured>"),
            )
            .field(
                "ca_certificate_path",
                &self.ca_certificate_path.as_ref().map(|_| "<configured>"),
            )
            .finish_non_exhaustive()
    }
}

/// Operator-granted access to one exact HTTPS issuer origin.
///
/// This is deliberately not a boolean: consumers cannot use it to disable the
/// shared private-network protections for unrelated hosts or redirects.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Url", into = "Url")]
pub struct TrustedIssuerOrigin(Url);

impl TrustedIssuerOrigin {
    pub fn new(url: Url) -> Result<Self, AuthError> {
        if url.scheme() != "https"
            || url.cannot_be_a_base()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(AuthError::Config(
                "trusted Authelia origin must be an exact HTTPS origin".to_string(),
            ));
        }

        let mut normalized = url;
        normalized.set_path("");
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl std::fmt::Debug for TrustedIssuerOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TrustedIssuerOrigin")
            .field(&origin_label(&self.0))
            .finish()
    }
}

impl TryFrom<Url> for TrustedIssuerOrigin {
    type Error = AuthError;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TrustedIssuerOrigin> for Url {
    fn from(value: TrustedIssuerOrigin) -> Self {
        value.0
    }
}

fn origin_label(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use super::{AutheliaConfig, TrustedIssuerOrigin};
    use url::Url;

    #[test]
    fn authelia_debug_redacts_secret_and_issuer_path() {
        let config = AutheliaConfig {
            issuer_url: Url::parse("https://auth.example.test/secret/path").unwrap(),
            client_id: "labby".to_string(),
            client_secret: "do-not-print".to_string(),
            trusted_private_origin: None,
            ca_certificate_path: None,
        };

        let debug = format!("{config:?}");
        assert!(debug.contains("https://auth.example.test"));
        assert!(!debug.contains("do-not-print"));
        assert!(!debug.contains("secret/path"));
    }

    #[test]
    fn private_trust_accepts_only_an_exact_https_origin() {
        assert!(TrustedIssuerOrigin::new(Url::parse("https://auth.lan:9443").unwrap()).is_ok());
        for invalid in [
            "http://auth.lan",
            "https://user@auth.lan",
            "https://auth.lan/path",
            "https://auth.lan/?query=secret",
            "https://auth.lan/#fragment",
        ] {
            assert!(TrustedIssuerOrigin::new(Url::parse(invalid).unwrap()).is_err());
        }
    }
}
