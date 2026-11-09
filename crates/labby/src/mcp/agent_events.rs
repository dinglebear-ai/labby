//! MCP Events over the authenticated root Streamable HTTP route.
use std::sync::Arc;

use rmcp::{
    RoleServer,
    model::{CustomRequest, CustomResult, ErrorCode, ErrorData},
    service::RequestContext,
};
use serde::de::DeserializeOwned;

use crate::dispatch::{
    agent_events::{
        EventAuthority, EventError, EventsListParams, EventsSubscribeParams,
        EventsUnsubscribeParams,
    },
    agent_messages::MessageEnrollment,
    codemode_notices::NoticeError,
};

use super::{context, notice_identity::notice_recipient, server::LabMcpServer};

pub(super) fn is_method(method: &str) -> bool {
    matches!(
        method,
        "events/list" | "events/subscribe" | "events/unsubscribe"
    )
}

impl LabMcpServer {
    pub(super) fn events_visible(&self) -> bool {
        self.transport_label == "http"
            && self.route_scope.is_root()
            && self.gateway_manager.is_some()
            && self.route_runtime.agent_events.is_some()
    }

    pub(super) async fn handle_event_request(
        &self,
        request: CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        if !self.events_visible() {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "Events are not enabled on this route",
                None,
            ));
        }
        let auth = context::auth_context_from_extensions(&context.extensions);
        let identity = context::verified_identity_from_extensions(&context.extensions);
        if auth.is_none_or(|auth| auth.via_session) || identity.is_none() {
            return Err(event_error(NoticeError::Identity.into()));
        }
        if !context::code_mode_read_scope_allowed(auth) {
            return Err(event_error(NoticeError::Identity.into()));
        }
        if request.method == "events/list" {
            let value = parse_params::<EventsListParams>(request.params)?
                .list(self.route_runtime.http_messages.is_some())
                .map_err(event_error)?;
            return Ok(CustomResult::new(value));
        }
        if !context::tool_execute_scope_allowed(auth) {
            return Err(event_error(NoticeError::Identity.into()));
        }
        let events = self
            .route_runtime
            .agent_events
            .as_ref()
            .ok_or_else(|| event_error(NoticeError::Unavailable.into()))?;
        let value = match request.method.as_str() {
            "events/subscribe" => {
                let params = parse_params::<EventsSubscribeParams>(request.params)?;
                let (recipient, authority, enrollment) = if params.session_messages() {
                    let (enrollment, authority) = self.event_message_enrollment(context).await?;
                    (
                        enrollment.binding.recipient.clone(),
                        authority,
                        Some(enrollment),
                    )
                } else {
                    let recipient = notice_recipient(self, context)
                        .ok_or_else(|| event_error(NoticeError::Identity.into()))?;
                    let authority = self
                        .event_authority(identity.expect("checked before event scope"), context)
                        .ok_or_else(|| event_error(NoticeError::Identity.into()))?;
                    (recipient, authority, None)
                };
                events
                    .subscribe(recipient, Arc::new(authority), params, enrollment)
                    .await
            }
            "events/unsubscribe" => {
                let params = parse_params::<EventsUnsubscribeParams>(request.params)?;
                let recipient = if params.session_messages() {
                    let (enrollment, _) = self.event_message_enrollment(context).await?;
                    enrollment.binding.recipient
                } else {
                    notice_recipient(self, context)
                        .ok_or_else(|| event_error(NoticeError::Identity.into()))?
                };
                events.unsubscribe(recipient, params).await
            }
            _ => {
                return Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "Unknown event method",
                    None,
                ));
            }
        }
        .map_err(event_error)?;
        Ok(CustomResult::new(value))
    }

    async fn event_message_enrollment(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<(MessageEnrollment, EventAuthority), ErrorData> {
        let (binding, authority) = self
            .http_message_binding(context)
            .await
            .map_err(|error| event_error(error.into()))?;
        let enrollment = self
            .route_runtime
            .code_mode_notifications
            .registered_message_recipient(binding)
            .await
            .map_err(|error| event_error(error.into()))?;
        Ok((enrollment, authority))
    }

    fn event_authority(
        &self,
        identity: &labby_auth::VerifiedIdentity,
        context: &RequestContext<RoleServer>,
    ) -> Option<EventAuthority> {
        match identity.authenticator() {
            labby_auth::Authenticator::StaticBearer => Some(EventAuthority::StaticBearer),
            labby_auth::Authenticator::OauthBearer => context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| {
                    parts
                        .extensions
                        .get::<labby_auth::auth_context::VerifiedOAuthGrant>()
                })
                .cloned()
                .map(EventAuthority::OAuth),
            labby_auth::Authenticator::ProductCredential => {
                let adapter = self.route_runtime.event_credential_adapter.clone()?;
                let grant = context::bound_access_grant_from_extensions(&context.extensions)?;
                Some(EventAuthority::Product(
                    adapter,
                    Box::new(labby_auth::types::ProjectSessionBinding::from(grant)),
                ))
            }
            _ => None,
        }
    }
}

fn parse_params<T: DeserializeOwned>(params: Option<serde_json::Value>) -> Result<T, ErrorData> {
    serde_json::from_value(params.unwrap_or_else(|| serde_json::json!({})))
        .map_err(|_| event_error(EventError::Invalid))
}

fn event_error(error: EventError) -> ErrorData {
    match error {
        EventError::Callback(reason) => ErrorData::new(
            ErrorCode(-32015),
            "CallbackEndpointError",
            Some(serde_json::json!({"reason": reason})),
        ),
        EventError::Invalid | EventError::UnsupportedDelivery => {
            ErrorData::invalid_params(error.to_string(), None)
        }
        EventError::Store(error) => ErrorData::new(
            ErrorCode(-32000),
            error.to_string(),
            Some(serde_json::json!({"kind":error.kind()})),
        ),
    }
}
