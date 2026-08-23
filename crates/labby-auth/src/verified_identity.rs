//! Verified authentication facts used to link transport credentials to Principals.

use thiserror::Error;

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

/// Verified identity plus the mechanism that authenticated this request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    authenticator: Authenticator,
    principal_link: PrincipalLink,
}

impl VerifiedIdentity {
    /// Construct a verified external-provider identity.
    pub fn external(
        authenticator: Authenticator,
        issuer: impl Into<String>,
        subject: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let issuer = issuer.into();
        if issuer.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptyIssuer);
        }
        let parsed_issuer =
            url::Url::parse(&issuer).map_err(|_| VerifiedIdentityError::InvalidIssuer)?;
        if parsed_issuer.scheme() != "https" || parsed_issuer.host_str().is_none() {
            return Err(VerifiedIdentityError::InvalidIssuer);
        }
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptySubject);
        }
        Ok(Self {
            authenticator,
            principal_link: PrincipalLink::External { issuer, subject },
        })
    }

    /// Construct a verified local/service credential identity.
    pub fn local_credential(
        authenticator: Authenticator,
        credential_id: impl Into<String>,
    ) -> Result<Self, VerifiedIdentityError> {
        let credential_id = credential_id.into();
        if credential_id.trim().is_empty() {
            return Err(VerifiedIdentityError::EmptyCredentialId);
        }
        Ok(Self {
            authenticator,
            principal_link: PrincipalLink::LocalCredential { credential_id },
        })
    }

    /// Authentication mechanism used for this request.
    #[must_use]
    pub const fn authenticator(&self) -> Authenticator {
        self.authenticator
    }

    /// Stable Principal-link key, independent of authentication mechanism.
    #[must_use]
    pub const fn principal_link(&self) -> &PrincipalLink {
        &self.principal_link
    }
}

/// Invalid verified-identity input from an authentication boundary.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum VerifiedIdentityError {
    /// Canonical provider issuer was empty.
    #[error("verified identity issuer must not be empty")]
    EmptyIssuer,
    /// Canonical provider issuer was not an absolute HTTPS URL.
    #[error("verified identity issuer must be an absolute HTTPS URL")]
    InvalidIssuer,
    /// Stable provider subject was empty.
    #[error("verified identity subject must not be empty")]
    EmptySubject,
    /// Stable local credential identifier was empty.
    #[error("verified identity credential ID must not be empty")]
    EmptyCredentialId,
}
