//! LLM-backed Agent executor using the same OpenAI-compatible backend as Phoenix.

use std::time::Duration;

use labby_primitives::digest::Sha256Digest;
use labby_runtime::{
    agent_runtime::{
        AgentExecutionOutput, AgentExecutionRequest, AgentExecutor, AgentRuntimeError,
        ExecutionGuard, system_now_millis,
    },
    authority::AuthoritySafeBoundary,
};

use crate::{
    access::AccessStore,
    dispatch::{
        agent_payloads::{AgentPayloadStore, MAX_TASK_INPUT_BYTES},
        error::ToolError,
        phoenix_openai::{BASE_URL_ENV, OpenAiBackend},
    },
};

#[derive(Clone)]
pub(crate) struct LlmAgentExecutor {
    backend: OpenAiBackend,
    harness_digest: String,
    payloads: AgentPayloadStore,
    input: String,
}

pub(crate) fn current_harness_digest() -> Result<String, ToolError> {
    let backend = OpenAiBackend::from_env().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "unavailable".into(),
        message: format!(
            "Agent execution requires the shared OpenAI-compatible backend configured with {BASE_URL_ENV}"
        ),
    })?;
    Ok(harness_digest_for(&backend))
}

fn harness_digest_for(backend: &OpenAiBackend) -> String {
    Sha256Digest::of(format!("labby-openai-compatible-v1:{}", backend.base_url()).as_bytes())
        .to_string()
}

impl LlmAgentExecutor {
    pub(crate) fn new(store: &AccessStore, input: impl Into<String>) -> Result<Self, ToolError> {
        let input = input.into();
        if input.len() > MAX_TASK_INPUT_BYTES {
            return Err(ToolError::InvalidParam {
                message: format!("input must contain at most {MAX_TASK_INPUT_BYTES} UTF-8 bytes"),
                param: "input".into(),
            });
        }
        let backend = OpenAiBackend::from_env().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unavailable".into(),
            message: format!(
                "Agent execution requires the shared OpenAI-compatible backend configured with {BASE_URL_ENV}"
            ),
        })?;
        let harness_digest = harness_digest_for(&backend);
        Ok(Self {
            backend,
            harness_digest,
            payloads: AgentPayloadStore::for_access_store(store),
            input,
        })
    }

    pub(crate) fn from_task(store: &AccessStore, input_digest: &str) -> Result<Self, ToolError> {
        let payloads = AgentPayloadStore::for_access_store(store);
        let input = payloads.load_task_input(input_digest)?;
        let backend = OpenAiBackend::from_env().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unavailable".into(),
            message: format!(
                "Agent execution requires the shared OpenAI-compatible backend configured with {BASE_URL_ENV}"
            ),
        })?;
        let harness_digest = harness_digest_for(&backend);
        Ok(Self {
            backend,
            harness_digest,
            payloads,
            input,
        })
    }

    pub(crate) fn output(&self, digest: &str) -> Result<String, ToolError> {
        self.payloads.load_output(digest)
    }
}

impl AgentExecutor for LlmAgentExecutor {
    async fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        if request.definition.revision.harness_digest != self.harness_digest {
            return Err(AgentRuntimeError::PinnedDefinitionMismatch);
        }
        let payload = self
            .payloads
            .load_agent(&request.definition.revision.content_digest)
            .map_err(|error| execution_error(&request, "load_payload", error))?;
        let prompt = render_prompt(
            &request.definition.id,
            request.definition.revision.version,
            &payload.instructions,
            &self.input,
        );
        let session_id = request.session.session_id.clone();

        guard
            .check(
                AuthoritySafeBoundary::BeforeExternalEffect,
                system_now_millis(),
            )
            .await?;
        self.backend
            .create_session(&session_id)
            .await
            .map_err(|error| execution_error(&request, "create_session", error))?;

        if let Err(error) = guard
            .check(
                AuthoritySafeBoundary::BeforeExternalEffect,
                system_now_millis(),
            )
            .await
        {
            // The provider session already exists at this point. Cleanup is part
            // of the execution lifecycle, not detached best-effort work that can
            // be lost during runtime shutdown.
            cancel_and_close(&self.backend, &session_id).await;
            return Err(error);
        }

        let chat = self.backend.chat(&session_id, &payload.model, &prompt);
        tokio::pin!(chat);
        let output = loop {
            tokio::select! {
                result = &mut chat => {
                    match result {
                        Ok(output) => break output,
                        Err(error) => {
                            drop(self.backend.close_session(&session_id).await);
                            return Err(execution_error(&request, "chat", error));
                        }
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if let Err(error) = guard
                        .check(
                            AuthoritySafeBoundary::BeforeExternalEffect,
                            system_now_millis(),
                        )
                        .await
                    {
                        cancel_and_close(&self.backend, &session_id).await;
                        return Err(error);
                    }
                }
            }
        };

        if let Err(error) = guard
            .check(AuthoritySafeBoundary::BeforeCommit, system_now_millis())
            .await
        {
            drop(self.backend.close_session(&session_id).await);
            return Err(error);
        }
        let digest = match self.payloads.store_output(&output) {
            Ok(digest) => digest,
            Err(error) => {
                drop(self.backend.close_session(&session_id).await);
                return Err(execution_error(&request, "store_output", error));
            }
        };
        if let Err(error) = self.backend.close_session(&session_id).await {
            tracing::warn!(
                agent_id = %request.definition.id,
                session_id = %session_id,
                kind = error.kind(),
                "Agent execution completed but provider session close failed"
            );
        }
        Ok(AgentExecutionOutput {
            digest,
            bytes: output.len(),
            external_effects: 3,
        })
    }
}

fn render_prompt(agent_id: &str, version: u64, instructions: &str, input: &str) -> String {
    if input.trim().is_empty() {
        return format!(
            "You are executing Labby Agent {agent_id} revision {version}. Follow the pinned agent instructions exactly.\n\nAgent instructions:\n{instructions}"
        );
    }
    format!(
        "You are executing Labby Agent {agent_id} revision {version}. Follow the pinned agent instructions exactly.\n\nAgent instructions:\n{instructions}\n\nRun input:\n{input}"
    )
}

async fn cancel_and_close(backend: &OpenAiBackend, session_id: &str) {
    drop(backend.cancel_session(session_id).await);
    drop(backend.close_session(session_id).await);
}

fn execution_error(
    request: &AgentExecutionRequest,
    stage: &'static str,
    error: ToolError,
) -> AgentRuntimeError {
    tracing::warn!(
        agent_id = %request.definition.id,
        session_id = %request.session.session_id,
        stage,
        kind = error.kind(),
        "LLM Agent executor failed"
    );
    AgentRuntimeError::ExecutorFailed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_pinned_instructions_and_run_input() {
        let prompt = render_prompt("agent-1", 7, "Be concise.", "Summarize this.");
        assert!(prompt.contains("Agent agent-1 revision 7"));
        assert!(prompt.contains("Be concise."));
        assert!(prompt.contains("Summarize this."));
    }
}
