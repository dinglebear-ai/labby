#[cfg(feature = "http-axum")]
use crate::authelia::AutheliaProvider;
use crate::config::{AutheliaConfig, GoogleConfig, InboundProviderKind};
#[cfg(feature = "http-axum")]
use crate::error::AuthError;
#[cfg(feature = "http-axum")]
use crate::google::AuthorizeUrlRequest;
use crate::google::GoogleProvider;
use serde::{Deserialize, Serialize};
#[cfg(feature = "http-axum")]
use url::Url;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderExchange {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub hosted_domain: Option<String>,
    #[serde(skip_serializing)]
    pub access_token: String,
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub granted_scopes: Vec<String>,
    #[serde(skip_serializing)]
    pub id_token: Option<String>,
}

impl std::fmt::Debug for ProviderExchange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderExchange")
            .field("subject", &"<redacted>")
            .field("email", &self.email.as_ref().map(|_| "<redacted>"))
            .field("email_verified", &self.email_verified)
            .field(
                "hosted_domain",
                &self.hosted_domain.as_ref().map(|_| "<redacted>"),
            )
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("granted_scopes", &self.granted_scopes)
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Validated, closed inbound-provider configuration for version 1.
///
/// Runtime dispatch is concrete and exhaustive; this intentionally does not
/// expose a provider registry or dynamic trait boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboundProvider {
    Google(GoogleConfig),
    Authelia(AutheliaConfig),
}

/// Closed runtime dispatch for inbound human identity providers.
///
/// Google retains its established credential-broker implementation while
/// Authelia uses its OIDC verifier and local-policy refresh path.
/// Keeping this enum closed prevents inbound provider behavior from leaking
/// into the separate upstream OAuth credential-store abstraction.
#[derive(Clone, Debug)]
pub enum InboundProviderRuntime {
    Google(Box<GoogleProvider>),
    #[cfg(feature = "http-axum")]
    Authelia(Box<AutheliaProvider>),
}

impl InboundProviderRuntime {
    #[must_use]
    pub const fn kind(&self) -> InboundProviderKind {
        match self {
            Self::Google(_) => InboundProviderKind::Google,
            #[cfg(feature = "http-axum")]
            Self::Authelia(_) => InboundProviderKind::Authelia,
        }
    }

    #[must_use]
    #[cfg(test)]
    #[allow(clippy::panic)]
    pub fn google(&self) -> &GoogleProvider {
        match self {
            Self::Google(provider) => provider,
            #[cfg(feature = "http-axum")]
            Self::Authelia(_) => panic!("Google provider requested while Authelia is active"),
        }
    }

    #[must_use]
    pub const fn google_provider(&self) -> Option<&GoogleProvider> {
        match self {
            Self::Google(provider) => Some(provider),
            #[cfg(feature = "http-axum")]
            Self::Authelia(_) => None,
        }
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        match self {
            Self::Google(provider) => &provider.client_id,
            #[cfg(feature = "http-axum")]
            Self::Authelia(provider) => &provider.client_id,
        }
    }

    #[must_use]
    pub fn issuer(&self) -> &str {
        match self {
            Self::Google(_) => crate::google::GOOGLE_ISSUER,
            #[cfg(feature = "http-axum")]
            Self::Authelia(provider) => provider.issuer(),
        }
    }

    #[cfg(feature = "http-axum")]
    pub fn authorize_url(&self, request: &AuthorizeUrlRequest) -> Result<Url, AuthError> {
        match self {
            Self::Google(provider) => provider.authorize_url(request),
            Self::Authelia(provider) => provider.authorize_url(request),
        }
    }

    #[cfg(feature = "http-axum")]
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        state: &str,
    ) -> Result<ProviderExchange, AuthError> {
        match self {
            Self::Google(provider) => {
                provider
                    .exchange_code(code, verifier)
                    .await
                    .map(|exchange| ProviderExchange {
                        subject: exchange.subject,
                        email: exchange.email,
                        email_verified: exchange.email_verified,
                        hosted_domain: exchange.hosted_domain,
                        access_token: exchange.access_token,
                        refresh_token: exchange.refresh_token,
                        expires_in: exchange.expires_in,
                        granted_scopes: exchange.granted_scopes,
                        id_token: exchange.id_token,
                    })
            }
            Self::Authelia(provider) => {
                provider
                    .exchange_code(code, verifier, &crate::util::fingerprint(state))
                    .await
            }
        }
    }
}

impl InboundProvider {
    #[must_use]
    pub const fn kind(&self) -> InboundProviderKind {
        match self {
            Self::Google(_) => InboundProviderKind::Google,
            Self::Authelia(_) => InboundProviderKind::Authelia,
        }
    }
}
