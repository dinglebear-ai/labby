//! Nonsecret identity references for explicitly authorized durable work.
//! These are loaded only from the trusted access store, never from request identity JSON.
use labby_auth::VerifiedIdentity;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub(crate) struct DurableIdentityReference {
    authenticator: String,
    transport_issuer: String,
    issuer: Option<String>,
    subject: Option<String>,
    credential_id: Option<String>,
}
impl DurableIdentityReference {
    pub(crate) fn capture(identity: &VerifiedIdentity) -> Self {
        use labby_auth::{Authenticator, PrincipalLink};
        let authenticator = match identity.authenticator() {
            Authenticator::BrowserSession => "browser",
            Authenticator::OauthBearer => "oauth",
            Authenticator::StaticBearer => "static",
            Authenticator::ProductCredential => "product",
            Authenticator::UnixPeer => "unix",
        }
        .to_owned();
        let (issuer, subject, credential_id) = match identity.principal_link() {
            PrincipalLink::External { issuer, subject } => {
                (Some(issuer.clone()), Some(subject.clone()), None)
            }
            PrincipalLink::LocalCredential { credential_id } => {
                (None, None, Some(credential_id.clone()))
            }
        };
        Self {
            authenticator,
            transport_issuer: identity.transport_credential_issuer().to_owned(),
            issuer,
            subject,
            credential_id,
        }
    }
    pub(crate) fn restore(self) -> super::error::AccessStoreResult<VerifiedIdentity> {
        use labby_auth::Authenticator;
        let auth = match self.authenticator.as_str() {
            "browser" => Authenticator::BrowserSession,
            "oauth" => Authenticator::OauthBearer,
            "static" => Authenticator::StaticBearer,
            "product" => Authenticator::ProductCredential,
            "unix" => Authenticator::UnixPeer,
            _ => return Err(super::AccessStoreError::MalformedVocabulary),
        };
        let link = match (self.credential_id, self.issuer, self.subject) {
            (Some(credential_id), None, None) => {
                labby_auth::PrincipalLink::LocalCredential { credential_id }
            }
            (None, Some(issuer), Some(subject)) => {
                labby_auth::PrincipalLink::External { issuer, subject }
            }
            _ => return Err(super::AccessStoreError::MalformedVocabulary),
        };
        VerifiedIdentity::for_durable_delegation(auth, self.transport_issuer, link)
            .map_err(|_| super::AccessStoreError::MalformedVocabulary)
    }
}
