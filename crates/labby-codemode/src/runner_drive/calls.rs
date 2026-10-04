use super::*;

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
    tool_deadline: tokio::time::Instant,
    execution_start: std::time::Instant,
    cfg: &RunnerConfig,
    cancellation: &CancellationToken,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) {
    let call_id = id.clone();
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
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
                caller,
                surface,
                &capability_filter,
                ctx,
                &cancellation,
            )
            .await;
        let elapsed_ms = call_start.elapsed().as_millis();
        (seq, call_id, redacted_params, result, elapsed_ms, start_ms)
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
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) {
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
    let caller = cfg.caller.clone();
    let capability_filter = cfg.capability_filter.clone();
    let execution_id = cfg.execution_id.clone();
    let trace_context = cfg.trace_context.clone();
    let openapi_registry = cfg.openapi_registry.clone();
    let openapi_http_client = cfg.openapi_http_client.clone();
    pending_tool_calls.push(Box::pin(async move {
        let start_ms = execution_start.elapsed().as_millis();
        let call_start = std::time::Instant::now();
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
            return (
                seq,
                id,
                redacted_params,
                Err(error),
                call_start.elapsed().as_millis(),
                start_ms,
            );
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
                    return (
                        seq,
                        id,
                        redacted_params,
                        Ok(ToolCallOutcome { value, ui: None }),
                        call_start.elapsed().as_millis(),
                        start_ms,
                    );
                }
                StepDecision::Execute => {}
                StepDecision::Error { kind, message } => {
                    let error = CodeModeCallError::new(kind, message).with_tool(id.clone());
                    return (
                        seq,
                        id,
                        redacted_params,
                        Err(error),
                        call_start.elapsed().as_millis(),
                        start_ms,
                    );
                }
            }
        }
        let dispatched = if matches!(local.provider, LocalProviderName::Sandbox) {
            // Disposable workloads have their own admission/lifecycle; never hold
            // the shared filesystem/git provider lock during guest execution.
            crate::sandbox::dispatch(&local.method, params).await
        } else if matches!(local.provider, LocalProviderName::Openapi) {
            // NO LOCAL_PROVIDER_LOCK — openapi has no shared mutable local state,
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
            let _guard = LOCAL_PROVIDER_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .await;
            dispatch_local_provider_stub(local, params).await
        };
        // Record applied (marker; ephemeral re-executes on replay). A record
        // failure fails the run closed inside the host hook.
        if let (Some(host), Ok(value)) = (broker.host, &dispatched)
            && let Err(err) = host.record_local(ctx, value).await
        {
            let error = CodeModeCallError::from(err).with_tool(id.clone());
            return (
                seq,
                id,
                redacted_params,
                Err(error),
                call_start.elapsed().as_millis(),
                start_ms,
            );
        }
        let result = dispatched
            .map(|value| ToolCallOutcome { value, ui: None })
            .map_err(|error| CodeModeCallError::from(error).with_tool(id.clone()));
        (
            seq,
            id,
            redacted_params,
            result,
            call_start.elapsed().as_millis(),
            start_ms,
        )
    }));
}

/// Dispatch an `openapi::<label>.<operationId>` call to `labby-openapi`. Splits
/// the method on the FIRST `.` so a dotted operationId (`vendor.pets.list`) is
/// preserved. Runs OUTSIDE `LOCAL_PROVIDER_LOCK`.
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
    pending_tool_calls.push(Box::pin(async move {
        let error = CodeModeCallError::from(err).with_tool(id.clone());
        (seq, id, redacted_params, Err(error), 0, 0)
    }));
}

/// Settle an over-ceiling `__lab_internal::*` pseudo-tool call with the
/// fail-open empty semantic result — the same `{"ranked": []}` shape
/// `dispatch_internal_call` returns when the embedding service is degraded —
/// so `codemode.search()` silently falls back to lexical-only scoring.
///
/// This deliberately resolves (never rejects) the in-sandbox promise:
/// FAIL-OPEN is a hard Code Mode invariant, so exceeding the internal-call
/// ceiling must not error or hang the run. The call never reaches
/// `host.semantic_rank` (that is the point — no further embedding-service
/// round trips) and, being internal, never appears in the call trace.
pub(super) fn enqueue_internal_call_over_ceiling(
    seq: u64,
    id: String,
    params: Value,
    cfg: &RunnerConfig,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'_>>,
) {
    let redacted_params = crate::trace::redact_trace_params(&params, cfg.trace_params);
    // Shape-aware: each reserved internal tool has its own "nothing found"
    // contract its sandbox-side caller already handles gracefully —
    // `codemode.search`'s empty ranked list, `codemode.describe`'s null type
    // body. Settling every over-ceiling call with `semantic_rank`'s shape
    // would silently "work" for `describe_types` too today only by a JS
    // truthiness coincidence (`{ranked: []}.dts` is `undefined`, same falsy
    // outcome as `{dts: null}.dts`), not by design — keep this explicit so a
    // future third internal tool doesn't inherit that accident.
    let fail_open_value = if id == "__lab_internal::describe_types" {
        json!({ "dts": null })
    } else {
        json!({ "ranked": [] })
    };
    pending_tool_calls.push(Box::pin(async move {
        (
            seq,
            id,
            redacted_params,
            Ok(ToolCallOutcome {
                value: fail_open_value,
                ui: None,
            }),
            0,
            0,
        )
    }));
}

pub(super) async fn dispatch_local_provider_stub(
    local: LocalProviderCall,
    params: Value,
) -> Result<Value, ToolError> {
    drop(local.params);
    let provider_name = local.provider.as_str();
    match local.provider {
        LocalProviderName::State => {
            let workspace_root = labby_runtime::lab_home()
                .join("code-mode-workspaces")
                .join("default");
            let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::default())?;
            dispatch_state_method(&workspace, &local.method, params).await
        }
        LocalProviderName::Git => {
            let workspace_root = labby_runtime::lab_home()
                .join("code-mode-workspaces")
                .join("default");
            let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::default())?;
            let _ = provider_name;
            dispatch_git_method(&workspace, &local.method, params).await
        }
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
