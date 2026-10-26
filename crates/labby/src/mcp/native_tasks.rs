//! MCP Tasks projection of the durable, authorized Agent Task dispatcher.
//!
//! A protocol Task identifies the immutable job, including its owner, creation
//! time and route authority. Worker recovery remains part of that same job.
//! No executor, scheduler, output cache, or peer-local task registry lives here.

use base64::Engine as _;
use rmcp::model::{
    CallToolResult, DetailedTask, GetTaskResult, ProtocolVersion, Task, TaskPayload, TaskStatus,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::dispatch::error::ToolError;
use crate::mcp::server::LabMcpServer;

pub(crate) const NATIVE_TASK_PREFIX: &str = "labby:task:";
const TASK_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const POLL_INTERVAL_MS: u64 = 1000;

pub(crate) fn native_tasks_negotiated(context: &RequestContext<RoleServer>) -> bool {
    context
        .meta
        .protocol_version()
        .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        && context
            .meta
            .client_capabilities()
            .is_some_and(|capabilities| capabilities.supports_tasks())
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Task is unavailable".into(),
        required_scopes: Vec::new(),
    }
}

fn protocol_error(error: ToolError) -> ErrorData {
    match error.kind() {
        "forbidden" | "not_found" | "invalid_param" | "missing_param" => {
            ErrorData::invalid_params("task not found", None)
        }
        _ => ErrorData::internal_error(
            "Native task is unavailable",
            Some(json!({"kind":error.kind()})),
        ),
    }
}

fn native_id(id: &str) -> Result<String, ToolError> {
    let encoded = id
        .strip_prefix(NATIVE_TASK_PREFIX)
        .filter(|value| value.len() <= 1024)
        .and_then(|value| value.split_once(':'))
        .filter(|(identity, _)| identity.len() == 64)
        .map(|(_, encoded)| encoded)
        .ok_or_else(denied)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| denied())?;
    String::from_utf8(bytes).map_err(|_| denied())
}

impl LabMcpServer {
    async fn native_task_gate(
        &self,
        action: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<(), ToolError> {
        if !native_tasks_negotiated(context)
            || !self.route_scope.exposes_tools()
            || !self.route_scope.allows_service("tasks")
            || self.registry.dispatch_capability("tasks")
                != Some(crate::registry::DispatchCapability::CallerBound)
            || !self.service_visible_on_mcp("tasks").await
            || !self.action_allowed_on_mcp("tasks", action).await
        {
            return Err(denied());
        }
        // Native task tools are not authorized on Project AssetUse routes.
        // Keep tasks/* behind the same boundary as their originating tools/call.
        #[cfg(feature = "gateway")]
        if !matches!(
            crate::mcp::bound_access::project_execution_binding(
                &context.extensions,
                std::time::SystemTime::now(),
            ),
            crate::mcp::bound_access::ProjectExecutionBinding::Legacy
        ) {
            return Err(denied());
        }
        Ok(())
    }

    async fn native_task_summary(
        &self,
        id: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<Value, ToolError> {
        self.native_task_gate("tasks.get", context).await?;
        let summary = Box::pin(self.dispatch_caller_bound_service(
            "tasks",
            "tasks.get",
            json!({"task_id":id}),
            context,
            Some(&context.meta),
        ))
        .await?;
        if let Some(team) = self.route_scope.bound_team_id()
            && (summary["owner_kind"] != "team" || summary["owner_id"] != team)
        {
            return Err(denied());
        }
        Ok(summary)
    }

    fn native_task_metadata(&self, summary: &Value) -> Result<Task, ToolError> {
        let id = summary["task_id"].as_str().ok_or_else(denied)?;
        let created = summary["created_at"].as_i64().ok_or_else(denied)?;
        let updated = summary["updated_at"].as_i64().ok_or_else(denied)?;
        let now = labby_runtime::agent_runtime::system_now_millis();
        if u64::try_from(created)
            .ok()
            .is_none_or(|created| now.saturating_sub(created) >= TASK_TTL_MS)
        {
            return Err(denied());
        }
        let binding = json!([
            self.route_scope.notification_authority_key(),
            created,
            summary["owner_kind"],
            summary["owner_id"]
        ]);
        let digest = hex::encode(Sha256::digest(binding.to_string().as_bytes()));
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id);
        let timestamp = |millis| {
            jiff::Timestamp::from_millisecond(millis)
                .map(|value| value.to_string())
                .map_err(|_| denied())
        };
        Ok(Task::new(
            format!("{NATIVE_TASK_PREFIX}{digest}:{encoded}"),
            TaskStatus::Working,
            timestamp(created)?,
            timestamp(updated)?,
        )
        .with_ttl_ms(TASK_TTL_MS)
        .with_poll_interval_ms(POLL_INTERVAL_MS))
    }

    /// Capture a usable handle before admitting any work. The caller still
    /// executes the ordinary shared tasks.queue exactly once after this check.
    pub(crate) async fn native_task_seed(
        &self,
        params: &Value,
        context: &RequestContext<RoleServer>,
    ) -> Result<Task, ToolError> {
        self.native_task_gate("tasks.queue", context).await?;
        let id = params["task_id"].as_str().ok_or_else(denied)?;
        self.native_task_metadata(&self.native_task_summary(id, context).await?)
    }

    async fn resolve_native_task(
        &self,
        task_id: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<(String, Value, Task), ToolError> {
        let id = native_id(task_id)?;
        let summary = self.native_task_summary(&id, context).await?;
        let task = self.native_task_metadata(&summary)?;
        if task.task_id != task_id {
            return Err(denied());
        }
        Ok((id, summary, task))
    }

    pub(crate) async fn get_native_task(
        &self,
        task_id: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        let (id, summary, task) = self
            .resolve_native_task(task_id, context)
            .await
            .map_err(protocol_error)?;
        let payload =
            match summary["state"].as_str() {
                Some("created" | "queued" | "running" | "cancelling") => TaskPayload::Working,
                Some("cancelled") => TaskPayload::Cancelled,
                Some("succeeded" | "failed" | "expired") => {
                    self.native_task_gate("tasks.result", context)
                        .await
                        .map_err(protocol_error)?;
                    // Preserve the shared creator-only result authorization and CAS
                    // materialization checks; Team administration is not output access.
                    let result = Box::pin(self.dispatch_caller_bound_service(
                        "tasks",
                        "tasks.result",
                        json!({"task_id":id}),
                        context,
                        Some(&context.meta),
                    ))
                    .await
                    .map_err(protocol_error)?;
                    if summary["state"] == "succeeded" {
                        let result = CallToolResult::structured(
                            crate::mcp::envelope::build_success("tasks", "tasks.queue", &result),
                        );
                        TaskPayload::Completed {
                            result: serde_json::to_value(result)
                                .map_err(|_| {
                                    ErrorData::internal_error("Task result unavailable", None)
                                })?
                                .as_object()
                                .cloned()
                                .ok_or_else(|| {
                                    ErrorData::internal_error("Task result unavailable", None)
                                })?,
                        }
                    } else {
                        let error =
                            ErrorData::internal_error("Agent Task execution failed", Some(result));
                        TaskPayload::Failed {
                            error: serde_json::to_value(error)
                                .map_err(|_| {
                                    ErrorData::internal_error("Task result unavailable", None)
                                })?
                                .as_object()
                                .cloned()
                                .ok_or_else(|| {
                                    ErrorData::internal_error("Task result unavailable", None)
                                })?,
                        }
                    }
                }
                _ => return Err(ErrorData::invalid_params("task not found", None)),
            };
        Ok(GetTaskResult::new(DetailedTask::new(task, payload)))
    }

    pub(crate) async fn cancel_native_task(
        &self,
        task_id: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.native_task_gate("tasks.cancel", context)
            .await
            .map_err(protocol_error)?;
        let (id, _, _) = self
            .resolve_native_task(task_id, context)
            .await
            .map_err(protocol_error)?;
        Box::pin(self.dispatch_caller_bound_service(
            "tasks",
            "tasks.cancel",
            json!({"task_id":id}),
            context,
            Some(&context.meta),
        ))
        .await
        .map(drop)
        .map_err(protocol_error)
    }

    pub(crate) async fn update_native_task(
        &self,
        task_id: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.resolve_native_task(task_id, context)
            .await
            .map_err(protocol_error)?;
        Err(ErrorData::invalid_params(
            "Native Agent Tasks have no outstanding input requests",
            None,
        ))
    }
}

#[cfg(test)]
#[path = "native_tasks_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "native_tasks_wire_tests.rs"]
mod wire_tests;
