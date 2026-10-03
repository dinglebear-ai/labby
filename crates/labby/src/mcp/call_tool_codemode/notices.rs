//! MCP adaptation only: derive recipient authority, handle control inputs, and
//! attach advisory notices without replacing the user's result or error.
use crate::mcp::{
    context::{
        auth_context_from_extensions, authorized_client_id_from_extensions,
        openai_session_fingerprint,
    },
    server::LabMcpServer,
};
use crate::notifications::codemode::{
    NoticeBatch, NoticeConsumer, NoticeError, NoticeRecipient, NoticeStore,
    validate_acknowledgments,
};
use rmcp::{
    RoleServer,
    model::{CallToolResult, ContentBlock},
    service::RequestContext,
};
use serde_json::{Map, Value};

pub(super) fn recipient(
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
    let (actor, consumer) = match (auth, actor, client, credential) {
        (Some(_), Some(actor), Some(client), _) => (
            Some(actor.to_owned()),
            NoticeConsumer::Client {
                id: client.to_owned(),
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
        route: server.route_scope.label().to_owned(),
        consumer,
    })
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

pub(super) struct NoticeControl {
    store: NoticeStore,
    recipient: Option<NoticeRecipient>,
    fields: Map<String, Value>,
}

pub(super) async fn prepare(
    server: &LabMcpServer,
    args: &Map<String, Value>,
    context: &RequestContext<RoleServer>,
    read_only: bool,
) -> Result<NoticeControl, NoticeError> {
    let register = match args.get("notification_inbox") {
        Some(Value::Bool(v)) => *v,
        None => false,
        _ => return Err(NoticeError::Invalid),
    };
    let ids = match args.get("ack_notifications") {
        None => None,
        Some(value) => {
            let values = value
                .as_array()
                .filter(|v| v.len() <= crate::notifications::codemode::MAX_ACKS)
                .ok_or(NoticeError::Invalid)?;
            let ids = values
                .iter()
                .map(|v| {
                    v.as_str()
                        .filter(|s| s.len() == 33)
                        .map(ToOwned::to_owned)
                        .ok_or(NoticeError::Invalid)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Some(ids)
        }
    };
    if let Some(ids) = &ids {
        validate_acknowledgments(ids)?;
    }
    let recipient = recipient(server, context);
    let store = server.route_runtime.code_mode_notifications.clone();
    let mut fields = Map::new();
    if register || ids.is_some() {
        if read_only {
            return Err(NoticeError::ReadOnly);
        }
        let who = recipient.clone().ok_or(NoticeError::Identity)?;
        let (inbox, acknowledged) = store.controls(who, register, ids).await?;
        if let Some(inbox) = inbox {
            fields.insert(
                "notification_inbox".into(),
                serde_json::to_value(inbox).map_err(|_| NoticeError::Unavailable)?,
            );
        }
        if let Some(acknowledged) = acknowledged {
            fields.insert(
                "acknowledged_notifications".into(),
                serde_json::to_value(acknowledged).map_err(|_| NoticeError::Unavailable)?,
            );
        }
    }
    Ok(NoticeControl {
        store,
        recipient,
        fields,
    })
}

fn decorated(
    result: &CallToolResult,
    fields: &Map<String, Value>,
    batch: Option<&NoticeBatch>,
    max_bytes: usize,
    max_tokens: usize,
) -> Option<CallToolResult> {
    let mut fields = fields.clone();
    if let Some(batch) = batch {
        let Value::Object(batch) = serde_json::to_value(batch).ok()? else {
            return None;
        };
        fields.extend(batch);
    }
    if fields.is_empty() {
        return None;
    }
    let mut candidate = result.clone();
    let mut text: Value = serde_json::from_str(&candidate.content.first()?.as_text()?.text).ok()?;
    let structured = candidate.structured_content.as_mut()?.as_object_mut()?;
    let text_object = text.as_object_mut()?;
    for (key, value) in fields {
        if structured.contains_key(&key) || text_object.contains_key(&key) {
            return None;
        }
        structured.insert(key.clone(), value.clone());
        text_object.insert(key, value);
    }
    let ContentBlock::Text(block) = candidate.content.first_mut()? else {
        return None;
    };
    block.text = serde_json::to_string(&text).ok()?;
    let encoded = serde_json::to_string(&candidate).ok()?;
    if encoded.len() > max_bytes
        || crate::mcp::result_format::estimate_tokens(&encoded) > max_tokens
    {
        return None;
    }
    Some(candidate)
}

impl NoticeControl {
    pub(super) async fn attach(
        &self,
        result: &mut CallToolResult,
        max_bytes: usize,
        max_tokens: usize,
    ) {
        if let Some(who) = self.recipient.clone() {
            let original = result.clone();
            let fields = self.fields.clone();
            match self
                .store
                .deliver(who, move |batch| {
                    decorated(&original, &fields, Some(batch), max_bytes, max_tokens)
                })
                .await
            {
                Ok(Some(candidate)) => {
                    *result = candidate;
                    return;
                }
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    subsystem = "agent_notifications",
                    action = "defer",
                    kind = error.kind(),
                    "notices deferred; execution result preserved"
                ),
            }
        }
        if let Some(candidate) = decorated(result, &self.fields, None, max_bytes, max_tokens) {
            *result = candidate;
        }
    }
}

#[cfg(test)]
mod tests;
