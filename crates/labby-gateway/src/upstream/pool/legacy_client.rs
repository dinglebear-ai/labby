//! Client-handler wrapper used for legacy MCP initialization.
//!
//! The modern 2026-07-28 lifecycle carries the selected protocol version in
//! per-request metadata. Legacy `initialize` carries it in `ClientInfo`, so a
//! fallback connection must override only that field while preserving every
//! server-to-client callback implemented by the original handler.

use std::future::Future;

use rmcp::ClientHandler;
use rmcp::model::{ErrorData as McpError, *};
use rmcp::service::{MaybeSendFuture, NotificationContext, RequestContext, RoleClient};

#[derive(Clone, Debug)]
pub(super) struct VersionedClientHandler<H> {
    inner: H,
    protocol_version: ProtocolVersion,
}

impl<H> VersionedClientHandler<H> {
    pub(super) fn new(inner: H, protocol_version: ProtocolVersion) -> Self {
        Self {
            inner,
            protocol_version,
        }
    }
}

#[allow(deprecated)]
impl<H: ClientHandler> ClientHandler for VersionedClientHandler<H> {
    fn ping(
        &self,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
        self.inner.ping(context)
    }
    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + MaybeSendFuture + '_ {
        self.inner.create_message(params, context)
    }
    fn list_roots(
        &self,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, McpError>> + MaybeSendFuture + '_ {
        self.inner.list_roots(context)
    }
    fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ElicitResult, McpError>> + MaybeSendFuture + '_ {
        self.inner.create_elicitation(request, context)
    }
    fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CustomResult, McpError>> + MaybeSendFuture + '_ {
        self.inner.on_custom_request(request, context)
    }
    fn on_cancelled(
        &self,
        params: CancelledNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_cancelled(params, context)
    }
    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_progress(params, context)
    }
    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_logging_message(params, context)
    }
    fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_resource_updated(params, context)
    }
    fn on_resource_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_resource_list_changed(context)
    }
    fn on_tool_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_tool_list_changed(context)
    }
    fn on_prompt_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_prompt_list_changed(context)
    }
    fn on_subscriptions_acknowledged(
        &self,
        params: SubscriptionsAcknowledgedNotificationParams,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_subscriptions_acknowledged(params, context)
    }
    fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_task_status(params, context)
    }
    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.inner.on_custom_notification(notification, context)
    }
    fn get_info(&self) -> ClientInfo {
        let mut info = self.inner.get_info();
        info.protocol_version = self.protocol_version.clone();
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_only_the_protocol_version() {
        let mut original = ClientInfo::default();
        original.client_info.name = "compat-client".to_string();
        original.client_info.version = "9.1.0".to_string();
        let original_capabilities = original.capabilities.clone();
        let wrapped = VersionedClientHandler::new(original, ProtocolVersion::V_2025_11_25);
        let info = wrapped.get_info();
        assert_eq!(info.protocol_version, ProtocolVersion::V_2025_11_25);
        assert_eq!(info.client_info.name, "compat-client");
        assert_eq!(info.client_info.version, "9.1.0");
        assert_eq!(info.capabilities, original_capabilities);
    }
}
