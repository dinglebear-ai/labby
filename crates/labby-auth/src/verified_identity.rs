//! Verified authentication facts used to link transport credentials to Principals.

use std::collections::HashSet;

use thiserror::Error;

use crate::util::fingerprint;

const INITIAL_GENERATION: u64 = 1;

/// Authentication mechanism that produced a verified identity fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authenticator {
    /// Authenticated by a server-side browser session.
    BrowserSession,
    /// Authenticated by a Labby-issued OAuth bearer token.
    OauthBearer,
    /// Authenticated by a configured static bearer credential.
    StaticBearer,
    /// Authenticated by kernel-provided Unix peer credentials.
    UnixPeer,
}

impl Authenticator {
    const fn default_transport_issuer(self) -> &'static str {
        match self {
            Self::BrowserSession => "browser-session",
            Self::OauthBearer => "labby-jwt",
            Self::StaticBearer => "local",
            Self::UnixPeer => "unix-peer",
        }
    }
}

/// A canonical provider issuer admitted by the authentication boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TrustedIssuer(String);

/// Exact allowlist of canonical external identity-provider issuers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrustedIssuerRegistry {
    issuers: HashSet<String>,
}

impl TrustedIssuerRegistry {
    /// Construct an issuer allowlist, rejecting unsafe or ambiguous issuer URLs.
    fn new<I, S>(issuers: I) -> Result<Self, VerifiedIdentityError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let issuers = issuers
            .into_iter()
            .map(|issuer| canonicalize_issuer(issuer.as_ref()))
            .collect::<Result<HashSet<_>, _>>()?;
        if issuers.is_empty() {
            return Err(VerifiedIdentityError::EmptyIssuerRegistry);
        }
        Ok(Self { issuers })
    }

    /// Resolve an asserted issuer to its canonical, explicitly trusted value.
    fn resolve(&self, issuer: &str) -> Result<TrustedIssuer, VerifiedIdentityError> {
        let issuer = canonicalize_issuer(issuer)?;
        if !self.issuers.contains(&issuer) {
            return Err(VerifiedIdentityError::UntrustedIssuer);
        }
        Ok(TrustedIssuer(issuer))
    }

    fn google() -> Self {
        Self::new([crate::google::GOOGLE_ISSUER]).expect("the built-in Google issuer is valid")
    }
}

/// Stable identity key used by the access-control layer to link a Principal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrincipalLink {
    /// Identity asserted by a trusted external provider.
    External {
        /// Canonical provider issuer.
        issuer: String,
        /// Stable provider subject.
        subject: String,
    },
    /// Identity asserted by a stable local credential identifier.
    LocalCredential {
        /// Stable identifier assigned by the authentication boundary.
        credential_id: String,
    },
}

impl PrincipalLink {
    /// Stable redacted identifier for this already-validated durable link.
    ///
    /// This performs no issuer admission. It is suitable for checking a link
    /// that was validated before persistence without coupling reads to the
    /// process's current provider configuration.
    #[must_use]
    pub fn safe_fingerprint(&self) -> String {
        let material = match self {
            Self::External { issuer, subject } => format!("external\0{issuer}\0{subject}"),
            Self::LocalCredential { credential_id } => format!("local\0{credential_id}"),
        };
        fingerprint(&material)
    }
}

/// Verified identity plus the authentication facts that produced its link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    authenticator: Authenticator,
    transport_credential_issuer: String,
    principal_link: PrincipalLink,
}

impl VerifiedIdentity {
    /// Version of the boundary verification facts represented by this type.
    pub const VERIFICATION_SCHEMA_VERSION: u64 = INITIAL_GENERATION;
    /// Version of the durable Principal-link encoding represented by this type.
    pub const LINK_SCHEMA_VERSION: u64 = INITIAL_GENERATION;

    /// Construct a verified identity for the built-in trusted provider.
    pub fn external(
        authenticator: Authenticator,
        issuer: impl AsRef<str>,
        subject: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let transport_issuer = authenticator.default_transport_issuer();
        Self::external_with_transport_issuer(authenticator, transport_issuer, issuer, subject)
    }

    /// Construct a verified built-in-provider identity with its actual transport issuer.
    pub(crate) fn external_with_transport_issuer(
        authenticator: Authenticator,
        transport_credential_issuer: impl Into<String>,
        issuer: impl AsRef<str>,
        subject: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let issuer = TrustedIssuerRegistry::google().resolve(issuer.as_ref())?;
        Self::external_with_facts(authenticator, transport_credential_issuer, issuer, subject)
    }

    /// Construct an external identity from the configured provider allowlist.
    pub(crate) fn external_from_allowed_issuers<I, S>(
        authenticator: Authenticator,
        transport_credential_issuer: impl Into<String>,
        issuer: impl AsRef<str>,
        subject: impl Into<String>,
        allowed_issuers: I,
    ) -> Result<Self, VerifiedIdentityError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let issuer = TrustedIssuerRegistry::new(allowed_issuers)?.resolve(issuer.as_ref())?;
        Self::external_with_facts(authenticator, transport_credential_issuer, issuer, subject)
    }

    /// Construct a verified external identity from boundary-validated facts.
    fn external_with_facts(
        authenticator: Authenticator,
        transport_credential_issuer: impl Into<String>,
        issuer: TrustedIssuer,
        subject: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptySubject);
        }
        Self::from_link(
            authenticator,
            transport_credential_issuer,
            PrincipalLink::External {
                issuer: issuer.0,
                subject,
            },
        )
    }

    /// Construct a verified local/service credential identity.
    pub fn local_credential(
        authenticator: Authenticator,
        credential_id: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let transport_issuer = authenticator.default_transport_issuer();
        Self::local_credential_with_issuer(authenticator, transport_issuer, credential_id)
    }

    /// Construct a local identity with its actual transport credential issuer.
    pub fn local_credential_with_issuer(
        authenticator: Authenticator,
        transport_credential_issuer: impl Into<String>,
        credential_id: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let credential_id = credential_id.into();
        if credential_id.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptyCredentialId);
        }
        Self::from_link(
            authenticator,
            transport_credential_issuer,
            PrincipalLink::LocalCredential { credential_id },
        )
    }

    fn from_link(
        authenticator: Authenticator,
        transport_credential_issuer: impl Into<String>,
        principal_link: PrincipalLink,
    ) -> Result<Self, VerifiedIdentityError> {
        let transport_credential_issuer = transport_credential_issuer.into();
        if transport_credential_issuer.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptyTransportIssuer);
        }
        Ok(Self {
            authenticator,
            transport_credential_issuer,
            principal_link,
        })
    }

    /// Authentication mechanism used for this request.
    #[must_use]
    pub const fn authenticator(&self) -> Authenticator {
        self.authenticator
    }

    /// Issuer of the presented transport credential, not the identity provider.
    #[must_use]
    pub fn transport_credential_issuer(&self) -> &str {
        &self.transport_credential_issuer
    }

    /// Stable Principal-link key, independent of authentication mechanism.
    #[must_use]
    pub const fn principal_link(&self) -> &PrincipalLink {
        &self.principal_link
    }

    /// Stable redacted identifier suitable for logs and audit correlation.
    #[must_use]
    pub fn safe_fingerprint(&self) -> String {
        self.principal_link.safe_fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_canonicalization_is_exact_and_preserves_valid_paths() {
        let registry = TrustedIssuerRegistry::new(["https://LOGIN.EXAMPLE.com/"])
            .expect("trusted issuer registry should be valid");
        assert_eq!(
            registry.resolve("https://login.example.com"),
            Ok(TrustedIssuer("https://login.example.com".to_string()))
        );
        assert_eq!(
            registry.resolve("https://evil.example.com"),
            Err(VerifiedIdentityError::UntrustedIssuer)
        );
        let pathful = TrustedIssuerRegistry::new(["https://login.example.com/oidc/tenant/"])
            .expect("pathful OIDC issuer should be valid");
        assert_eq!(
            pathful.resolve("https://LOGIN.example.com/oidc/tenant"),
            Ok(TrustedIssuer(
                "https://login.example.com/oidc/tenant".to_string()
            ))
        );
        for invalid in [
            "http://login.example.com",
            "https://login.example.com?tenant=one",
            "https://user@login.example.com",
            "https://login.example.com/#tenant",
        ] {
            assert_eq!(
                canonicalize_issuer(invalid),
                Err(VerifiedIdentityError::InvalidIssuer),
                "unexpected canonicalization for {invalid}"
            );
        }
    }
}

fn canonicalize_issuer(issuer: &str) -> Result<String, VerifiedIdentityError> {
    if issuer.trim().is_empty() {
        return Err(VerifiedIdentityError::EmptyIssuer);
    }
    let parsed = url::Url::parse(issuer).map_err(|_| VerifiedIdentityError::InvalidIssuer)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(VerifiedIdentityError::InvalidIssuer);
    }
    let origin = parsed.origin().ascii_serialization();
    let path = parsed.path().trim_end_matches('/');
    Ok(format!("{origin}{path}"))
}

/// Invalid verified-identity input from an authentication boundary.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum VerifiedIdentityError {
    /// Canonical provider issuer was empty.
    #[error("verified identity issuer must not be empty")]
    EmptyIssuer,
    /// Canonical provider issuer was not a safe HTTPS issuer URI.
    #[error("verified identity issuer must be a safe HTTPS issuer URI")]
    InvalidIssuer,
    /// Canonical provider issuer was not in the trusted registry.
    #[error("verified identity issuer is not trusted")]
    UntrustedIssuer,
    /// Trusted issuer registry contained no issuers.
    #[error("trusted issuer registry must not be empty")]
    EmptyIssuerRegistry,
    /// Transport credential issuer was empty.
    #[error("transport credential issuer must not be empty")]
    EmptyTransportIssuer,
    /// Token provenance was absent or described more than one identity kind.
    #[error("verified identity provenance must describe exactly one identity kind")]
    InvalidIdentityProvenance,
    /// Stable provider subject was empty.
    #[error("verified identity subject must not be empty")]
    EmptySubject,
    /// Stable local credential identifier was empty.
    #[error("verified identity credential ID must not be empty")]
    EmptyCredentialId,
}
