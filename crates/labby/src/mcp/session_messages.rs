//! The messages tool binds transport authority to the durable message store.
mod local;
#[cfg(test)]
mod tests;

use axum::http::request::Parts;
use rmcp::{
    RoleServer,
    model::{CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, ErrorData},
    service::RequestContext,
};

use crate::dispatch::{
    agent_events::EventAuthority,
    agent_messages::{MessageRequest, SessionBinding},
    codemode_notices::{NoticeError, NoticeStore},
};

use super::server::LabMcpServer;
use crate::dispatch::agent_messages::http::HttpMessageRuntime;
pub(crate) use local::LocalMessageSession;

pub(crate) const TOOL_NAME: &str = "messages";

impl LabMcpServer {
    pub(super) fn messages_visible(&self, context: &RequestContext<RoleServer>) -> bool {
        if !self.route_scope.is_root()
            || self.registry.is_proxy_aggregate()
            || matches!(
                super::bound_access::project_execution_binding(
                    &context.extensions,
                    std::time::SystemTime::now()
                ),
                super::bound_access::ProjectExecutionBinding::Unavailable
            )
        {
            return false;
        }
        match self.transport_label {
            "http" if self.route_runtime.http_messages.is_some() => {
                context.extensions.get::<Parts>().is_some_and(|parts| {
                    HttpMessageRuntime::eligible(
                        parts,
                        self.route_runtime.event_credential_adapter.as_ref(),
                    )
                })
            }
            "stdio" => self
                .route_runtime
                .local_messages
                .as_ref()
                .is_some_and(|local| local.visible()),
            _ => false,
        }
    }

    pub(super) async fn http_message_binding(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<(SessionBinding, EventAuthority), NoticeError> {
        if self.transport_label != "http" || !self.messages_visible(context) {
            return Err(NoticeError::Identity);
        }
        let parts = context
            .extensions
            .get::<Parts>()
            .ok_or(NoticeError::Identity)?;
        let runtime = self
            .route_runtime
            .http_messages
            .as_ref()
            .ok_or(NoticeError::Identity)?;
        runtime
            .binding(parts, self.route_runtime.event_credential_adapter.as_ref())
            .await
    }

    pub(super) async fn call_messages(
        &self,
        request: CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !self.messages_visible(context) {
            return Err(hidden());
        }
        let binding = if let Some(local) = &self.route_runtime.local_messages {
            local.authorize().await
        } else {
            self.http_message_binding(context)
                .await
                .map(|(binding, _)| binding)
        }
        .map_err(tool_error)?;
        execute(
            &self.route_runtime.code_mode_notifications,
            binding,
            request,
        )
        .await
    }
}

pub(super) async fn execute(
    store: &NoticeStore,
    binding: SessionBinding,
    request: CallToolRequestParams,
) -> Result<CallToolResponse, ErrorData> {
    let input = serde_json::from_value::<MessageRequest>(serde_json::Value::Object(
        request.arguments.unwrap_or_default(),
    ))
    .map_err(|_| tool_error(NoticeError::Invalid))?;
    let result = store
        .message_request(binding, input)
        .await
        .map_err(tool_error)?;
    Ok(CallToolResponse::Complete(CallToolResult::structured(
        result,
    )))
}

pub(super) fn hidden() -> ErrorData {
    ErrorData::invalid_params("messages is unavailable for this connection", None)
}

pub(super) fn tool_error(error: NoticeError) -> ErrorData {
    let message = match error {
        NoticeError::Invalid => "invalid message action or parameters",
        NoticeError::NotFound => "message or active session unavailable",
        NoticeError::Identity | NoticeError::Forbidden | NoticeError::ReadOnly => {
            "verified message session required"
        }
        NoticeError::Conflict => "idempotency key belongs to a different message",
        NoticeError::Full => "message capacity reached",
        NoticeError::Busy => "message store busy",
        NoticeError::Unavailable => "message authority or store unavailable",
        NoticeError::Deadline => {
            "message request timed out; reconcile with the same idempotency key"
        }
    };
    ErrorData::new(
        ErrorCode(-32000),
        message,
        Some(serde_json::json!({"kind":error.kind()})),
    )
}
