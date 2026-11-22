use super::*;

/// Bind receipt metadata to the bounded, redacted trace representation.
fn trace_argument_digest(redacted: &Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(redacted).expect("JSON Value serialization is infallible");
    format!("trace-sha256:{}", hex::encode(Sha256::digest(bytes)))
}

// This future stays in the existing bounded pending-call set until its receipt
// is ready. Other runner frames and ready calls continue to drain during I/O.
async fn prepare_outcome(
    result: Result<ToolCallOutcome, CodeModeCallError>,
    seq: u64,
    id: &str,
    root: &Path,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Result<PreparedOutcome, CodeModeCallError> {
    let outcome = result?;
    let serialized_len =
        crate::protocol::serialized_len_within(&outcome.value, usize::MAX).unwrap_or(0);
    let receipt = if !protocol_boundary::is_internal_tool_id(id)
        && serialized_len > crate::response_artifacts::inline_threshold()
    {
        crate::response_artifacts::preserve(
            root,
            format!("automatic/tool-{seq}.json"),
            &outcome.value,
            caller,
            scope,
        )
        .await
    } else {
        None
    };
    Ok(PreparedOutcome {
        outcome,
        serialized_len,
        receipt,
    })
}

/// Enqueue a `ToolCall` request from the runner into `pending_tool_calls`.
///
/// Free function (not `&self` method) so the returned future can capture
/// `broker` with the same lifetime as the enclosing `run_in_runner_with_config`
/// rather than being forced to `'static`.
#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_tool_call<'a, H: CodeModeHost>(
    broker: &'a CodeModeBroker<'a, H>,
    seq: u64,
    call_ordinal: Option<u64>,
    id: String,
    params: Value,
    full_result: bool,
    tool_deadline: tokio::time::Instant,
    execution_start: std::time::Instant,
    cfg: &RunnerConfig,
    cancellation: &CancellationToken,
    artifact_root: PathBuf,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) {
    let call_id = id.clone();
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
    let params_digest = redacted_params.as_ref().map(trace_argument_digest);
    let caller = cfg.caller.clone();
    let capability_filter = cfg.capability_filter.clone();
    let surface = cfg.surface;
    let execution_id = cfg.execution_id.clone();
    let trace_context = cfg.trace_context.clone();
    let cancellation = cancellation.clone();
    pending_tool_calls.push(Box::pin(async move {
        let start_ms = execution_start.elapsed().as_millis();
        let call_start = std::time::Instant::now();
        let ctx = ExecCtx {
            seq,
            execution_id,
            call_ordinal,
            trace_context,
            step_ordinal: None,
        };
        let result = broker
            .call_tool_id_before_deadline(
                &id,
                params,
                tool_deadline,
                caller.clone(),
                surface,
                &capability_filter,
                ctx,
                &cancellation,
            )
            .await;
        let result = result.map(|mut outcome| {
            if full_result {
                // Hosts without a native MCP envelope expose an explicit host-value
                // shape instead; no fabricated MCP response or second dispatch.
                let full = outcome
                    .full_result
                    .clone()
                    .unwrap_or_else(|| json!({"host_value": outcome.value}));
                outcome.value = full;
            }
            outcome
        });
        let elapsed_ms = call_start.elapsed().as_millis();
        let result = prepare_outcome(
            result,
            seq,
            &call_id,
            &artifact_root,
            &caller,
            &capability_filter,
        )
        .await;
        (
            seq,
            call_id,
            redacted_params,
            params_digest,
            result,
            elapsed_ms,
            start_ms,
        )
    }));
}

/// Enqueue a runner-reserved LOCAL provider call (`state::*` / `git::*`).
///
/// The call is routed through `host.decide_local` / `host.record_local` so a
/// future replay implementation could journal it as an **ephemeral** entry.
/// The current host does not override these hooks, so the default impls make
/// this a no-op and the call dispatches directly.
///
/// Free function taking `broker` (not `&self`) so the future captures the
/// broker with the enclosing loop's lifetime rather than being forced to
/// `'static` (same pattern as `enqueue_tool_call`).
#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_local_provider_call<'a, H: CodeModeHost>(
    broker: &'a CodeModeBroker<'a, H>,
    seq: u64,
    call_ordinal: Option<u64>,
    id: String,
    local: LocalProviderCall,
    params: Value,
    execution_start: std::time::Instant,
    cfg: &RunnerConfig,
    tool_deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
    artifact_root: PathBuf,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) {
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
    let params_digest = redacted_params.as_ref().map(trace_argument_digest);
    let cancellation = cancellation.clone();
    let caller = cfg.caller.clone();
    let capability_filter = cfg.capability_filter.clone();
    let execution_id = cfg.execution_id.clone();
    let trace_context = cfg.trace_context.clone();
    let openapi_registry = cfg.openapi_registry.clone();
    let openapi_http_client = cfg.openapi_http_client.clone();
    pending_tool_calls.push(Box::pin(async move {
        let start_ms = execution_start.elapsed().as_millis();
        let call_start = std::time::Instant::now();
        let dispatch = async {
            let provider_allowed = if matches!(local.provider, LocalProviderName::Openapi) {
                crate::execute::openapi_provider_allowed(&caller, &capability_filter, &openapi_registry)
            } else {
                crate::execute::local_providers_allowed(&caller, &capability_filter)
            };
            if !provider_allowed {
                let error = CodeModeCallError::from(ToolError::Forbidden {
                    message: "local Code Mode provider is not available to this caller".to_string(),
                    required_scopes: vec!["lab:admin".to_string()],
                })
                .with_tool(id.clone());
                return Err(error);
            }
            let ctx = ExecCtx {
                seq,
                execution_id,
                call_ordinal,
                trace_context,
                step_ordinal: None,
            };
            // Reserved decision hook runs BEFORE dispatch. The default
            // `decide_local` returns Execute; the current host does not override it.
            if let Some(host) = broker.host {
                match host.decide_local(ctx.clone(), &id, &params).await {
                    StepDecision::Replay(value) => {
                        // Ephemeral entries never replay a stored result — but honor
                        // it defensively if a host ever returns one.
                        return Ok(ToolCallOutcome { full_result: None, value, ui: None });
                    }
                    StepDecision::Execute => {}
                    StepDecision::Error { kind, message } => {
                        let error = CodeModeCallError::new(kind, message).with_tool(id.clone());
                        return Err(error);
                    }
                }
            }
            let dispatched = if matches!(local.provider, LocalProviderName::Sandbox) {
                // Disposable workloads have their own admission/lifecycle; never hold
                // the shared filesystem/git provider lock during guest execution.
                crate::sandbox::dispatch(&local.method, params).await
            } else if matches!(local.provider, LocalProviderName::Openapi) {
                // No workspace lock — openapi has no shared mutable local state,
                // and must not serialize behind slow state/git ops. It still
                // participates in the reserved local-provider decision/record spine.
                dispatch_openapi_provider(
                    &openapi_registry,
                    &openapi_http_client,
                    broker.host,
                    &caller,
                    &capability_filter,
                    local,
                    params,
                )
                .await
            } else {
                dispatch_local_provider_stub(local, params, &caller).await
            };
            // Record applied (marker; ephemeral re-executes on replay). A record
            // failure fails the run closed inside the host hook.
            if let (Some(host), Ok(value)) = (broker.host, &dispatched)
                && let Err(err) = host.record_local(ctx, value).await
            {
                let error = CodeModeCallError::from(err).with_tool(id.clone());
                return Err(error);
            }
            dispatched
                .map(|value| ToolCallOutcome {
                    full_result: None,
                    value,
                    ui: None,
                })
                .map_err(|error| CodeModeCallError::from(error).with_tool(id.clone()))
        };
        let result = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(CodeModeCallError::new("cancelled", "local provider call cancelled").with_tool(id.clone())),
            result = tokio::time::timeout_at(tool_deadline, dispatch) => result.unwrap_or_else(|_| Err(CodeModeCallError::new("timeout", "local provider call timed out").with_tool(id.clone()))),
        };
        let elapsed_ms = call_start.elapsed().as_millis();
        let result = prepare_outcome(result, seq, &id, &artifact_root, &caller, &capability_filter).await;
        (
            seq,
            id,
            redacted_params,
            params_digest,
            result,
            elapsed_ms,
            start_ms,
        )
    }));
}

/// Dispatch an `openapi::<label>.<operationId>` call to `labby-openapi`. Splits
/// the method on the FIRST `.` so a dotted operationId (`vendor.pets.list`) is
/// preserved. Runs outside the state/Git workspace locks.
pub(super) async fn dispatch_openapi_provider<H: CodeModeHost>(
    registry: &labby_openapi::OpenApiRegistry,
    client: &reqwest::Client,
    host: Option<&H>,
    caller: &CodeModeCaller,
    scope: &ToolScope,
    local: LocalProviderCall,
    params: Value,
) -> Result<Value, ToolError> {
    let (label, op) = local
        .method
        .split_once('.')
        .ok_or_else(|| ToolError::InvalidParam {
            message: "openapi call must be openapi::<label>.<operationId>".to_string(),
            param: "id".to_string(),
        })?;
    let operation = registry.operation(label, op).map_err(ToolError::from)?;
    let credential = if operation.oauth_upstream.is_some() {
        if scope.is_scoped() || caller.subject().is_none_or(str::is_empty) {
            return Err(ToolError::Forbidden {
                message: "subject-scoped OpenAPI requires an authenticated unscoped caller".into(),
                required_scopes: vec!["lab".into()],
            });
        }
        let host = host.ok_or_else(|| ToolError::Forbidden {
            message: "subject-scoped OpenAPI requires a host-authenticated caller".into(),
            required_scopes: vec!["lab".into()],
        })?;
        Some(
            host.resolve_openapi_credential(label, op, caller)
                .await?
                .ok_or_else(|| ToolError::Forbidden {
                    message: "subject-scoped OpenAPI credential is unavailable".into(),
                    required_scopes: vec!["lab".into()],
                })?,
        )
    } else {
        if !crate::execute::local_providers_allowed(caller, scope) {
            return Err(ToolError::Forbidden {
                message: "static OpenAPI credentials require unscoped lab:admin".into(),
                required_scopes: vec!["lab:admin".into()],
            });
        }
        None
    };
    labby_openapi::dispatch_openapi_call_with_credential(
        registry, client, label, op, params, credential,
    )
    .await
    .map_err(Into::into)
}

pub(super) fn enqueue_rejected_tool_call(
    seq: u64,
    id: String,
    params: Value,
    err: ToolError,
    cfg: &RunnerConfig,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'_>>,
) {
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
    let params_digest = redacted_params.as_ref().map(trace_argument_digest);
    pending_tool_calls.push(Box::pin(async move {
        let error = CodeModeCallError::from(err).with_tool(id.clone());
        (seq, id, redacted_params, params_digest, Err(error), 0, 0)
    }));
}

pub(super) fn workspace_identity(
    caller: &CodeModeCaller,
) -> Result<(String, &'static Mutex<()>), ToolError> {
    let (name, stripe) = match caller.without_authority() {
        CodeModeCaller::TrustedLocal => ("default".to_owned(), 0),
        _ => {
            use sha2::{Digest, Sha256};
            let subject = caller
                .subject()
                .filter(|subject| !subject.is_empty())
                .ok_or_else(|| ToolError::Forbidden {
                    message: "an authenticated subject is required for a Code Mode workspace"
                        .into(),
                    required_scopes: Vec::new(),
                })?;
            let hash = Sha256::digest(subject.as_bytes());
            (
                hex::encode(hash),
                usize::from(hash[0]) % WORKSPACE_LOCKS.len(),
            )
        }
    };
    Ok((name, &WORKSPACE_LOCKS[stripe]))
}

pub(super) async fn dispatch_local_provider_stub(
    local: LocalProviderCall,
    params: Value,
    caller: &CodeModeCaller,
) -> Result<Value, ToolError> {
    drop(local.params);
    let (workspace_name, lock) = workspace_identity(caller)?;
    let _guard = lock.lock().await;
    let workspace_root = labby_runtime::lab_home()
        .join("code-mode-workspaces")
        .join(workspace_name);
    let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::default())?;
    match local.provider {
        LocalProviderName::State => dispatch_state_method(&workspace, &local.method, params).await,
        LocalProviderName::Git => dispatch_git_method(&workspace, &local.method, params).await,
        // `Openapi` is dispatched BEFORE the lock in `enqueue_local_provider_call`
        // and never reaches this stub. This arm is defensive only — a routing bug
        // returns a scrubbed internal error rather than silently sharing the lock.
        LocalProviderName::Openapi | LocalProviderName::Sandbox => Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "openapi provider must not be dispatched via the local-provider stub"
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_and_git_serialize_by_workspace_while_other_stripes_can_run() {
        let caller = |subject: &str| CodeModeCaller::Scoped {
            host_context: Default::default(),
            capabilities: crate::CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: true,
                is_admin: true,
            },
            sub: Some(subject.to_owned()),
        };
        let (alice, first) = workspace_identity(&caller("alice")).unwrap();
        let (same, second) = workspace_identity(&caller("alice")).unwrap();
        let (bob, other) = workspace_identity(&caller("bob")).unwrap();
        assert_eq!(alice, same);
        assert_ne!(alice, bob);
        let held = first.lock().await;
        assert!(second.try_lock().is_err());
        assert!(other.try_lock().is_ok());
        drop(held);
        assert!(second.try_lock().is_ok());
    }

    #[test]
    fn trace_argument_digest_uses_redacted_values() {
        let first = crate::trace::redact_trace_params(&json!({"password": "one"}), true).unwrap();
        let second = crate::trace::redact_trace_params(&json!({"password": "two"}), true).unwrap();
        let digest = trace_argument_digest(&first);
        assert!(digest.starts_with("trace-sha256:"));
        assert_eq!(digest, trace_argument_digest(&second));
    }
}
