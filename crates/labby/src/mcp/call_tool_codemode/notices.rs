//! MCP adaptation only: derive recipient authority, handle control inputs, and
//! attach advisory notices without replacing the user's result or error.
use crate::dispatch::codemode_notices::{
    NoticeBatch, NoticeError, NoticeRecipient, NoticeStore, validate_acknowledgments,
};
use crate::mcp::{notice_identity::notice_recipient, server::LabMcpServer};
use rmcp::{
    RoleServer,
    model::{CallToolResult, ContentBlock},
    service::RequestContext,
};
use serde_json::{Map, Value};

pub(crate) struct NoticeControl {
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
    prepare_for_recipient(
        server.route_runtime.code_mode_notifications.clone(),
        notice_recipient(server, context),
        args,
        read_only,
    )
    .await
}

pub(crate) async fn prepare_for_recipient(
    store: NoticeStore,
    recipient: Option<NoticeRecipient>,
    args: &Map<String, Value>,
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
                .filter(|v| v.len() <= crate::dispatch::codemode_notices::MAX_ACKS)
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
    pub(crate) async fn attach(
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
