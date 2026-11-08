//! The trusted recipient shared by pull and push notification delivery.
use super::{
    context::{
        auth_context_from_extensions, authorized_client_id_from_extensions,
        openai_session_fingerprint,
    },
    server::LabMcpServer,
};
use crate::dispatch::codemode_notices::{NoticeConsumer, NoticeRecipient};
use rmcp::{RoleServer, service::RequestContext};

pub(super) fn notice_recipient(
    server: &LabMcpServer,
    context: &RequestContext<RoleServer>,
) -> Option<NoticeRecipient> {
    let actor = server.request_actor_key(context);
    let client = authorized_client_id_from_extensions(&context.extensions);
    let auth = auth_context_from_extensions(&context.extensions);
    let identity = crate::mcp::context::verified_identity_from_extensions(&context.extensions);
    // Static bearer and product credentials have verified credential identities,
    // not OAuth azp IDs. Never accept clientInfo or request arguments as authority.
    let credential = identity.filter(|identity| {
        matches!(
            identity.authenticator(),
            labby_auth::Authenticator::StaticBearer | labby_auth::Authenticator::ProductCredential
        )
    });
    let conversation = openai_session_fingerprint(Some(&context.meta));
    if auth.is_none()
        && actor.is_none()
        && client.is_none()
        && credential.is_none()
        && server.transport_label == "stdio"
        && let Some(channel) = &server.route_runtime.claude_channel
    {
        return Some(channel.recipient.clone());
    }
    let (actor, consumer) = match (auth, actor, client, credential) {
        (Some(_), Some(actor), Some(client), _) => (
            Some(actor.to_owned()),
            NoticeConsumer::Client {
                id: oauth_client_binding(identity?, client)?,
                conversation,
            },
        ),
        (Some(_), _, None, Some(identity)) => {
            let (actor, id) = credential_binding(
                identity,
                crate::mcp::context::bound_access_grant_from_extensions(&context.extensions),
            )?;
            (Some(actor), NoticeConsumer::Credential { id, conversation })
        }
        (None, None, None, None) if server.transport_label == "stdio" => (
            None,
            NoticeConsumer::Stdio {
                session: server.route_runtime.notification_stdio_session().to_owned(),
            },
        ),
        _ => return None,
    };
    Some(NoticeRecipient {
        actor,
        route: server.route_scope.notification_authority_key(),
        consumer,
    })
}

fn oauth_client_binding(identity: &labby_auth::VerifiedIdentity, client: &str) -> Option<String> {
    use labby_auth::{Authenticator, PrincipalLink};
    use sha2::{Digest as _, Sha256};
    if identity.authenticator() != Authenticator::OauthBearer {
        return None;
    }
    let principal = match identity.principal_link() {
        PrincipalLink::External { issuer, subject } => {
            serde_json::json!(["external", issuer, subject])
        }
        PrincipalLink::LocalCredential { credential_id } => {
            serde_json::json!(["credential", credential_id])
        }
    };
    let encoded = serde_json::to_vec(&serde_json::json!([
        "labby.notice.oauth-client.v2",
        identity.transport_credential_issuer(),
        principal,
        client
    ]))
    .ok()?;
    Some(hex::encode(Sha256::digest(encoded)))
}

// Diagnostic fingerprints are deliberately truncated. Never use them as an
// authorization/storage namespace: hash complete verified, non-secret facts.
fn credential_binding(
    identity: &labby_auth::VerifiedIdentity,
    bound: Option<&labby_primitives::product_credential::BoundAccessGrant>,
) -> Option<(String, String)> {
    use sha2::{Digest as _, Sha256};
    let labby_auth::PrincipalLink::LocalCredential { credential_id } = identity.principal_link()
    else {
        return None;
    };
    let issuer = identity.transport_credential_issuer();
    let material = match identity.authenticator() {
        labby_auth::Authenticator::StaticBearer => {
            serde_json::json!(["static", issuer, credential_id])
        }
        labby_auth::Authenticator::ProductCredential => {
            let grant = bound?;
            if grant.credential_id != *credential_id || grant.issuer != issuer {
                return None;
            }
            let mut scopes = grant.scopes.clone();
            scopes.sort();
            scopes.dedup();
            serde_json::json!({"kind":"product","issuer":issuer,"credential":credential_id,
                "credential_generation":grant.credential_generation,"installation":grant.installation_id,
                "principal":grant.principal_id,"organization":grant.organization_id,"project":grant.project_id,
                "loadout":grant.loadout_id,"loadout_generation":grant.loadout_generation,
                "assignment_generation":grant.assignment_generation,"route":grant.route_id,
                "route_generation":grant.route_generation,"membership_epoch":grant.membership_epoch,
                "organization_policy_epoch":grant.organization_policy_epoch,"project_policy_epoch":grant.project_policy_epoch,
                "resource":grant.resource,"audience":grant.audience,"scopes":scopes})
        }
        _ => return None,
    };
    let principal = serde_json::to_vec(&serde_json::json!([
        "labby.notice.actor.v1",
        issuer,
        credential_id
    ]))
    .ok()?;
    let binding =
        serde_json::to_vec(&serde_json::json!(["labby.notice.credential.v1", material])).ok()?;
    Some((
        hex::encode(Sha256::digest(principal)),
        hex::encode(Sha256::digest(binding)),
    ))
}

#[cfg(test)]
#[test]
fn notice_production_full_credential_binding_and_reauthorization_boundary() {
    let static_identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "primary",
    )
    .unwrap();
    let (actor, binding) = credential_binding(&static_identity, None).unwrap();
    assert_eq!(actor.len(), 64);
    assert_eq!(binding.len(), 64);
    assert_eq!(
        credential_binding(&static_identity, None).unwrap(),
        (actor.clone(), binding.clone())
    );
    let other = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "secondary",
    )
    .unwrap();
    assert_ne!(credential_binding(&other, None).unwrap().1, binding);
    let product = labby_auth::VerifiedIdentity::local_credential_with_issuer(
        labby_auth::Authenticator::ProductCredential,
        "test-issuer",
        "cred",
    )
    .unwrap();
    assert!(credential_binding(&product, None).is_none());
    let mut grant = labby_primitives::product_credential::BoundAccessGrant {
        installation_id: "install".into(),
        issuer: "test-issuer".into(),
        subject: "subject".into(),
        principal_id: "principal".into(),
        organization_id: "org".into(),
        project_id: "project".into(),
        loadout_id: "loadout".into(),
        loadout_generation: 1,
        assignment_generation: 1,
        catalog_generation: 1,
        route_id: "route".into(),
        route_generation: 1,
        membership_epoch: 1,
        organization_policy_epoch: 1,
        project_policy_epoch: 1,
        credential_id: "cred".into(),
        credential_generation: 1,
        scopes: vec!["lab:read".into()],
        resource: "resource".into(),
        audience: "audience".into(),
        expires_at: 100,
        requires_admin: false,
        destructive: false,
    };
    let original = credential_binding(&product, Some(&grant)).unwrap();
    grant.expires_at += 1;
    grant.catalog_generation += 1;
    assert_eq!(
        credential_binding(&product, Some(&grant)).unwrap(),
        original,
        "renewal/catalog freshness are not a new authority"
    );
    grant.project_id = "another".into();
    assert_ne!(
        credential_binding(&product, Some(&grant)).unwrap(),
        original,
        "project rebinding must isolate inboxes"
    );
    grant.credential_id = "foreign".into();
    assert!(credential_binding(&product, Some(&grant)).is_none());
}

#[cfg(test)]
#[tokio::test]
async fn oauth_provider_change_cannot_inherit_an_inbox() {
    use crate::dispatch::codemode_notices::{
        NoticeLevel, NoticeProducer, NoticeStore, PublishNotice,
    };
    use labby_auth::config::{AuthConfig, EnterpriseIssuerConfig};
    let second_issuer = "https://another-idp.example";
    let config = AuthConfig {
        enterprise_issuers: vec![EnterpriseIssuerConfig {
            issuer: second_issuer.to_owned(),
            jwks_uri: None,
            jwks: None,
            allowed_client_ids: Vec::new(),
        }],
        ..AuthConfig::default()
    };
    let who = |issuer: &str| {
        let claims = labby_auth::jwt::AccessClaims {
            iss: "https://labby.example".into(),
            sub: "same-opaque-subject".into(),
            azp: "same-client".into(),
            identity_issuer: Some(issuer.to_owned()),
            ..Default::default()
        };
        let verified = labby_auth::verified_identity_from_access_claims(&claims, &config).unwrap();
        NoticeRecipient {
            actor: Some("same-observability-actor".into()),
            route: "root".into(),
            consumer: NoticeConsumer::Client {
                id: oauth_client_binding(&verified, &claims.azp).unwrap(),
                conversation: None,
            },
        }
    };
    let store = NoticeStore::memory().unwrap();
    let first = who("https://accounts.google.com");
    let second = who(second_issuer);
    let inbox = store.register(first.clone()).await.unwrap();
    assert_ne!(inbox.id, store.register(second.clone()).await.unwrap().id);
    let notice = store
        .publish(
            NoticeProducer {
                actor: "fixture".into(),
                admin: true,
            },
            PublishNotice {
                inbox_id: inbox.id,
                source: "ci".into(),
                level: NoticeLevel::Info,
                message: "Private build notice.".into(),
                dedupe_key: "one".into(),
                ttl_seconds: 3600,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .deliver(second, |batch| Some(batch.notifications.len()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .deliver(first, |batch| Some(batch.notifications[0].id.clone()))
            .await
            .unwrap(),
        Some(notice.id)
    );
}
