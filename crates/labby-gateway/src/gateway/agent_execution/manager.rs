use labby_runtime::error::ToolError;
use tokio_util::sync::CancellationToken;

use super::{
    AgentApprovalRequest, AgentExecuteRequest, AgentExecutionReceipt, AgentExecutionStatus,
    ApprovalChallenge, BoundContext, DelegationReceipt, ExecutionContextCreateRequest,
    ExecutionContextReceipt, Reservation, canonical_args_hash, now_ms,
};
use crate::gateway::manager::GatewayManager;
use crate::gateway::palette::{PaletteCaller, PaletteExecuteRequest};

impl GatewayManager {
    pub fn issue_actor_delegation(
        &self,
        actor: &str,
        audience: &str,
        scopes: &[String],
    ) -> Result<DelegationReceipt, ToolError> {
        self.agent_executions
            .issue_delegation(actor, audience, scopes)
    }

    pub async fn create_agent_execution_context(
        &self,
        service: &str,
        request: ExecutionContextCreateRequest,
    ) -> Result<ExecutionContextReceipt, ToolError> {
        let actor = self
            .agent_executions
            .delegation_actor(service, &request.delegation_token)?;
        self.execution_loadout_revision_contains(
            &actor,
            service,
            &request.loadout_id,
            request.loadout_revision,
            None,
            None,
        )
        .await?;
        self.agent_executions.create_context(
            service,
            &request.delegation_token,
            &request.loadout_id,
            request.loadout_revision,
            request.expires_at_unix_ms,
        )
    }

    pub async fn issue_agent_approval(
        &self,
        actor: &str,
        request: AgentApprovalRequest,
    ) -> Result<ApprovalChallenge, ToolError> {
        let context = self
            .agent_executions
            .bound_context_for_actor(&request.execution_context_id, actor)?;
        self.execution_loadout_revision_contains(
            &context.actor,
            &context.service,
            &context.loadout_id,
            context.loadout_revision,
            Some(&request.id),
            Some(&request.expected_contract_hash),
        )
        .await?;
        let caller = delegated_caller(&context, None);
        let descriptor = self.palette_descriptor(&caller, &request.id).await?;
        if descriptor.contract_hash != request.expected_contract_hash {
            return Err(contract_changed());
        }
        if !descriptor.destructive {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_params".into(),
                message: "approval challenges are issued only for destructive tools".into(),
            });
        }
        let args_hash = canonical_args_hash(&request.params)?;
        self.agent_executions.issue_approval(
            &request.execution_context_id,
            &context.service,
            &request.id,
            &args_hash,
            &request.expected_contract_hash,
        )
    }

    pub async fn execute_agent_tool(
        &self,
        service: &str,
        request: AgentExecuteRequest,
    ) -> Result<AgentExecutionReceipt, ToolError> {
        let context = self
            .agent_executions
            .bound_context(&request.execution_context_id, service)?;
        self.execution_loadout_revision_contains(
            &context.actor,
            &context.service,
            &context.loadout_id,
            context.loadout_revision,
            Some(&request.id),
            Some(&request.expected_contract_hash),
        )
        .await?;
        let caller = delegated_caller(&context, Some(&request.idempotency_key));
        let descriptor = self.palette_descriptor(&caller, &request.id).await?;
        if descriptor.contract_hash != request.expected_contract_hash {
            return Err(contract_changed());
        }
        let remaining_ms = request.deadline_at_unix_ms.saturating_sub(now_ms());
        if remaining_ms <= 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "deadline_exceeded".into(),
                message: "delegated execution deadline has elapsed".into(),
            });
        }
        let args_hash = canonical_args_hash(&request.params)?;
        match self.agent_executions.reserve(
            &request.execution_context_id,
            service,
            &request.idempotency_key,
            &request.id,
            &args_hash,
            &request.expected_contract_hash,
            request.approval_token.as_deref(),
            descriptor.destructive,
        )? {
            Reservation::Existing(receipt) | Reservation::Running(receipt) => return Ok(receipt),
            Reservation::Execute {
                receipt_id,
                audit_id,
            } => {
                tracing::info!(surface="api", service="agent_execution", action="execute", request_id=%request.idempotency_key, receipt_id, audit_id, actor=%context.actor, delegated_service=service, loadout_id=%context.loadout_id, loadout_revision=context.loadout_revision, tool_id=%request.id, contract_hash=%request.expected_contract_hash, "delegated exact execution reserved")
            }
        }
        let cancellation = CancellationToken::new();
        self.agent_execution_cancellations
            .insert(request.idempotency_key.clone(), cancellation.clone());
        let call = self.palette_execute_with_consumed_approval(
            &caller,
            PaletteExecuteRequest {
                id: request.id.clone(),
                params: request.params,
                expected_contract_hash: request.expected_contract_hash,
                confirm_destructive: descriptor.destructive,
            },
            descriptor.destructive,
        );
        let outcome = tokio::select! {
            biased;
            () = cancellation.cancelled() => self.agent_executions.finish(&request.idempotency_key, AgentExecutionStatus::Cancelled, None, Some("cancelled")),
            () = tokio::time::sleep(std::time::Duration::from_millis(remaining_ms as u64)) => self.agent_executions.finish(&request.idempotency_key, AgentExecutionStatus::TimedOut, None, Some("deadline_exceeded")),
            result = call => match result {
                Ok(response) => self.agent_executions.finish(&request.idempotency_key, AgentExecutionStatus::Succeeded, Some(&response.result), None),
                Err(error) => self.agent_executions.finish(&request.idempotency_key, AgentExecutionStatus::Failed, None, Some(error.kind())),
            }
        };
        self.agent_execution_cancellations
            .remove(&request.idempotency_key);
        outcome
    }

    pub fn agent_execution_status(
        &self,
        service: &str,
        request_id: &str,
    ) -> Result<AgentExecutionReceipt, ToolError> {
        let receipt = self
            .agent_executions
            .status(request_id)?
            .ok_or_else(not_found)?;
        if receipt.service != service {
            return Err(not_found());
        }
        Ok(receipt)
    }

    pub fn cancel_agent_execution(
        &self,
        service: &str,
        request_id: &str,
    ) -> Result<AgentExecutionReceipt, ToolError> {
        let receipt = self.agent_execution_status(service, request_id)?;
        if receipt.status != AgentExecutionStatus::Running {
            return Ok(receipt);
        }
        if let Some(token) = self.agent_execution_cancellations.get(request_id) {
            token.cancel();
            return Ok(receipt);
        }
        self.agent_executions.finish(
            request_id,
            AgentExecutionStatus::Interrupted,
            None,
            Some("interrupted"),
        )
    }
}

fn delegated_caller(context: &BoundContext, request_id: Option<&str>) -> PaletteCaller {
    let allowed = context
        .scopes
        .iter()
        .filter_map(|scope| scope.strip_prefix("gateway:"))
        .filter(|name| !name.is_empty() && *name != "*")
        .map(ToOwned::to_owned)
        .collect();
    PaletteCaller::scoped(&context.actor, request_id, context.scopes.clone(), allowed)
}

fn contract_changed() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "contract_changed".into(),
        message: "tool contract changed before delegated execution".into(),
    }
}
fn not_found() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".into(),
        message: "execution receipt was not found".into(),
    }
}
