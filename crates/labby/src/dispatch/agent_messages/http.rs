use std::sync::Arc;

use axum::http::{HeaderMap, HeaderName, header::AUTHORIZATION, request::Parts};
use labby_auth::{
    Authenticator, PrincipalLink, VerifiedIdentity,
    auth_context::{AuthContext, AuthorizedClientId, VerifiedOAuthGrant},
    middleware::parse_bearer_token,
    types::ProjectSessionBinding,
};
use labby_primitives::product_credential::BoundAccessGrant;
use zeroize::Zeroizing;

use crate::{
    access::AccessCredentialAdapter,
    dispatch::{
        agent_events::{EventAuthority, require_current},
        agent_messages::{
            SessionBinding, SessionKind,
            proof::{
                BridgeProof, BridgeProofRequest, MessageKey, SESSION_KEY_HEADER, canonical_digest,
            },
        },
        codemode_notices::{NoticeConsumer, NoticeError, NoticeRecipient, now_ms},
    },
};

/// Request-time authority for the opt-in root TCP listener.
pub(crate) struct HttpMessageRuntime {
    key: MessageKey,
    installation: String,
    daemon_epoch: String,
}

impl HttpMessageRuntime {
    pub(crate) fn new(key: MessageKey, installation: String) -> Self {
        Self {
            key,
            installation,
            daemon_epoch: ulid::Ulid::new().to_string(),
        }
    }

    pub(crate) fn eligible(parts: &Parts, adapter: Option<&Arc<AccessCredentialAdapter>>) -> bool {
        session_key(&parts.headers).is_some() && VerifiedCaller::from_parts(parts, adapter).is_ok()
    }

    pub(crate) async fn binding(
        &self,
        parts: &Parts,
        adapter: Option<&Arc<AccessCredentialAdapter>>,
    ) -> Result<(SessionBinding, EventAuthority), NoticeError> {
        let session_key = session_key(&parts.headers).ok_or(NoticeError::Identity)?;
        let caller = VerifiedCaller::from_parts(parts, adapter)?;
        let (owner_key, credential) = self.namespace(&caller)?;
        require_current(&caller.authority).await?;
        let binding_key = self.key.digest(
            "http-session",
            &[
                owner_key.as_bytes(),
                credential.as_bytes(),
                session_key.as_bytes(),
            ],
        );
        Ok((
            SessionBinding {
                recipient: NoticeRecipient {
                    actor: Some(owner_key.clone()),
                    route: "root".into(),
                    consumer: NoticeConsumer::MessageSession {
                        binding: binding_key.clone(),
                    },
                },
                owner_key,
                binding_key,
                kind: SessionKind::Http,
                stdio_closed: None,
            },
            caller.authority,
        ))
    }

    pub(crate) async fn prove_bridge(
        &self,
        parts: &Parts,
        adapter: Option<&Arc<AccessCredentialAdapter>>,
        request: &BridgeProofRequest,
    ) -> Result<BridgeProof, NoticeError> {
        let caller = VerifiedCaller::from_parts(parts, adapter)?;
        request.verify(&self.key, &caller.bearer, &self.installation, now_ms())?;
        let (owner_key, _) = self.namespace(&caller)?;
        require_current(&caller.authority).await?;
        BridgeProof::issue(
            &self.key,
            request,
            &caller.bearer,
            &owner_key,
            &self.daemon_epoch,
            now_ms(),
        )
    }

    fn namespace(&self, caller: &VerifiedCaller<'_>) -> Result<(String, String), NoticeError> {
        match &caller.authority {
            EventAuthority::StaticBearer => {
                Ok((self.key.static_owner(&caller.bearer), String::new()))
            }
            EventAuthority::OAuth(_) => {
                let PrincipalLink::External { issuer, subject } = caller.identity.principal_link()
                else {
                    return Err(NoticeError::Identity);
                };
                let owner_key = self.key.digest(
                    "oauth-owner",
                    &[
                        caller.identity.transport_credential_issuer().as_bytes(),
                        issuer.as_bytes(),
                        subject.as_bytes(),
                    ],
                );
                Ok((
                    owner_key,
                    caller.client.ok_or(NoticeError::Identity)?.to_owned(),
                ))
            }
            EventAuthority::Product(_, grant) => {
                let owner_key = self.key.digest(
                    "product-owner",
                    &[
                        grant.installation_id.as_bytes(),
                        grant.issuer.as_bytes(),
                        grant.principal_id.as_bytes(),
                        grant.organization_id.as_bytes(),
                        grant.project_id.as_bytes(),
                        grant.route_id.as_bytes(),
                    ],
                );
                // Full authority generations (including catalog) fence the binding.
                // Token expiry is verified on every request, not part of durable identity.
                let mut binding = grant.as_ref().clone();
                binding.source_credential_expires_at = 0;
                binding.scopes.sort();
                binding.scopes.dedup();
                let encoded = serde_json::to_vec(&binding).map_err(|_| NoticeError::Identity)?;
                Ok((owner_key, self.key.digest("product-grant", &[&encoded])))
            }
        }
    }
}

struct VerifiedCaller<'a> {
    bearer: Zeroizing<String>,
    identity: &'a VerifiedIdentity,
    client: Option<&'a str>,
    authority: EventAuthority,
}

impl<'a> VerifiedCaller<'a> {
    fn from_parts(
        parts: &'a Parts,
        adapter: Option<&Arc<AccessCredentialAdapter>>,
    ) -> Result<Self, NoticeError> {
        let auth = parts
            .extensions
            .get::<AuthContext>()
            .ok_or(NoticeError::Identity)?;
        if auth.via_session
            || !auth
                .scopes
                .iter()
                .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
        {
            return Err(NoticeError::Identity);
        }
        let bearer = one_header(&parts.headers, AUTHORIZATION)
            .and_then(parse_bearer_token)
            .map(Zeroizing::new)
            .ok_or(NoticeError::Identity)?;
        let identity = parts
            .extensions
            .get::<VerifiedIdentity>()
            .ok_or(NoticeError::Identity)?;
        let client = parts
            .extensions
            .get::<AuthorizedClientId>()
            .map(|id| id.0.as_ref());
        let authority = match identity.authenticator() {
            Authenticator::StaticBearer => EventAuthority::StaticBearer,
            Authenticator::OauthBearer => {
                if client.is_none_or(str::is_empty)
                    || !matches!(identity.principal_link(), PrincipalLink::External { .. })
                {
                    return Err(NoticeError::Identity);
                }
                EventAuthority::OAuth(
                    parts
                        .extensions
                        .get::<VerifiedOAuthGrant>()
                        .cloned()
                        .ok_or(NoticeError::Identity)?,
                )
            }
            Authenticator::ProductCredential => {
                let grant = parts
                    .extensions
                    .get::<BoundAccessGrant>()
                    .ok_or(NoticeError::Identity)?;
                if grant.route_id != "root"
                    || grant.issuer != identity.transport_credential_issuer()
                    || !matches!(identity.principal_link(), PrincipalLink::LocalCredential { credential_id }
                        if *credential_id == grant.credential_id)
                {
                    return Err(NoticeError::Identity);
                }
                EventAuthority::Product(
                    adapter.cloned().ok_or(NoticeError::Identity)?,
                    Box::new(ProjectSessionBinding::from(grant)),
                )
            }
            _ => return Err(NoticeError::Identity),
        };
        Ok(Self {
            bearer,
            identity,
            client,
            authority,
        })
    }
}

fn one_header(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).into_iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(value)
    }
}

fn session_key(headers: &HeaderMap) -> Option<&str> {
    one_header(headers, HeaderName::from_static(SESSION_KEY_HEADER))
        .filter(|key| canonical_digest(key))
}
