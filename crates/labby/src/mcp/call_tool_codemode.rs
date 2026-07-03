//! Code Mode gateway tool branch of `call_tool`.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as inherent
//! `impl LabMcpServer` helpers. Each helper is reached only after the
//! service-name match in `call_tool_impl` and self-`return`s its result.
//! Owns the single definition of the Code Mode tool description renderer, plus
//! `string_array_arg`.
//!
//! This branch logs via `tracing` directly (not `emit_dispatch_notification`)
//! and fires `notify_catalog_changes` around the broker call.

use std::collections::BTreeSet;
use std::time::Instant;

use labby_codemode::CodeModeExecutedCall;
use labby_codemode::{MAX_SOURCE_BYTES, SERVICE as CODE_MODE_SERVICE};
use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolResult, Content, JsonObject, Meta};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError as DispatchToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCallerCapabilities, CodeModeExecutionSource,
    CodeModeHistoryEntry, CodeModeHistoryKind, ToolScope, code_mode_execute_trace,
};
use crate::mcp::context::{auth_context_from_extensions, tool_execute_scope_allowed};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::result_format::{
    estimate_tokens, estimate_tokens_args, hash_arguments, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

/// Static body for the primary `codemode` MCP tool description.
///
/// The final model-visible description is rendered with the current upstream
/// namespace snapshot by [`code_mode_description`]. Keep the rendered result
/// under 8192 bytes.
pub(crate) const CODE_MODE_DESCRIPTION_BODY: &str = "\
Execute JavaScript in a sandbox with access to the Labby gateway catalog.

## Workflow

1. Discover: `const hits = await codemode.search({ query: \"short intent phrase\", limit: 5 });`
2. Inspect: `const docs = await codemode.describe(hits.results[0].path);`
3. Call: `await codemode.<upstream>.<tool>(params)` or `await callTool(\"upstream::tool\", params);`

Never guess helper or method names. If you have not already confirmed the exact \
tool, run `codemode.search(...)` first. `codemode.search` returns compact \
signatures; `codemode.describe(\"upstream.tool\")` returns focused TypeScript \
declarations and call details.

Pass `code` as `async () => { ... }` — the sandbox awaits its return value. \
Whatever it returns becomes `result`.

```ts
async () => {
  const hits = await codemode.search({ query: 'github issues', limit: 1 });
  const docs = await codemode.describe(hits.results[0].path);
  const issues = await codemode.github.search_issues({ q: 'bug' });
  return { tool: docs.path, count: issues.items.length };
}
```

Available globals: `codemode`, `callTool`, and `writeArtifact`. There is no \
`require`, `process`, `fs`, `fetch`, Node.js, Deno, or Bun API. All external I/O \
goes through gateway tools.

Optional top-level inputs to this MCP tool:
- `upstreams`: restrict this run to specific upstream namespaces.
- `tools`: restrict this run to specific tools; accepts raw tool names or \
`upstream::tool` ids.

Every upstream MCP tool is callable two ways: `callTool(id, params)`, or the \
auto-generated `codemode.<upstream>.<tool>(params)` helper (a thin wrapper over \
the same callTool, named from the live catalog). Snippets are discoverable \
through `codemode.search` and `codemode.describe`; run them with \
`codemode.run(\"<snippet>\", input)`.

`Promise.all([...])` dispatches `callTool` requests in parallel — batch independent \
reads instead of awaiting serially.

```ts
// codemode.<upstream>.<tool>() helpers are auto-generated from the live catalog.
// Use codemode.search() / codemode.describe() for compact docs, and callTool for
// dynamic ids.
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Successful return: the upstream tool's structuredContent if present, else the parsed \
text of the first content[0] block. Never the raw MCP envelope.

Error handling:
```ts
// To recover: const env: CodeModeError = JSON.parse(String(e.message));
// Retry-safe:    rate_limited (honor retry_after_ms), timeout, network_error
// Fix-and-retry: missing_param, invalid_param, validation_failed, confirmation_required
// Terminal:      unknown_tool, unknown_action, auth_failed, server_error, internal_error
```
A failed callTool rejects only its own promise — the run continues, so catch it and \
proceed. For catch-and-continue fan-out, prefer `Promise.allSettled` so every call \
settles before you return.

Scope: `codemode` requires `lab` or `lab:admin`.

Results are capped to the configured envelope budget (default 24 KB / 6000 tokens). \
Oversized results are replaced with a truncation marker containing `truncated`, \
`original_size`, `original_tokens`, `preview`, and `next_action`. Reduce data inside \
the sandbox before returning — that is the point of Code Mode.

Budget:
- Time: a 30 s wall-clock timeout bounds the whole run. Split work across \
calls or reduce local computation if the `timeout` kind is returned.
- Tool calls: default 512 `callTool` calls per run, configurable by the host up \
to 2048. Extra tool calls reject with `call_budget_exceeded`.
- Memory: 64 MiB heap limit enforced by the QuickJS runtime. Reduce the data \
processed inside the sandbox if the runner exits with `server_error`.
- Stack: QuickJS enforces a native stack depth limit; avoid deep recursion.
- The only recoverable budget kind is `timeout` — retry with a smaller payload \
or split into multiple `codemode` calls.

Lab actions (`lab::*` tool IDs) are not available in Code Mode. For Lab built-in \
actions, use the native Lab service tools instead of Code Mode.";

pub(crate) const CODE_MODE_DESCRIPTION_MAX_BYTES: usize = 8192;

fn code_mode_call_metrics_json(calls: &[CodeModeExecutedCall]) -> String {
    let calls = calls
        .iter()
        .map(|call| {
            let (namespace, tool) = call.id.split_once("::").unwrap_or(("", call.id.as_str()));
            serde_json::json!({
                "id": call.id,
                "namespace": namespace,
                "tool": tool,
                "ok": call.ok,
                "elapsed_ms": call.elapsed_ms,
                "error_kind": call.error_kind,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&calls).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeUpstreamDescription {
    pub(crate) name: String,
    pub(crate) hint: Option<String>,
}

fn push_description_line(out: &mut String, line: &str) -> bool {
    if out.len() + line.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES {
        out.push_str(line);
        true
    } else {
        false
    }
}

fn append_truncation_marker(out: &mut String, omitted_count: usize) {
    let marker = format!("- {omitted_count} more omitted; use codemode.search\n");
    while out.len() + marker.len() > CODE_MODE_DESCRIPTION_MAX_BYTES {
        if out.pop().is_none() {
            break;
        }
    }
    out.push_str(&marker);
}

#[must_use]
pub(crate) fn code_mode_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    let mut out = format!("{CODE_MODE_DESCRIPTION_BODY}\n\n## Available upstream namespaces\n\n");
    if upstreams.is_empty() {
        push_description_line(&mut out, "- none currently configured\n");
        return out.trim_end().to_string();
    }
    for (idx, upstream) in upstreams.iter().enumerate() {
        let line = match upstream
            .hint
            .as_deref()
            .and_then(labby_runtime::gateway_config::normalize_code_mode_hint)
        {
            Some(hint) => format!("- `{}` -- {}\n", upstream.name, hint),
            None => format!("- `{}`\n", upstream.name),
        };
        if !push_description_line(&mut out, &line) {
            append_truncation_marker(&mut out, upstreams.len() - idx);
            tracing::warn!(
                omitted_count = upstreams.len() - idx,
                max_bytes = CODE_MODE_DESCRIPTION_MAX_BYTES,
                "code mode upstream namespace description truncated"
            );
            break;
        }
    }
    out.trim_end().to_string()
}

pub(crate) fn string_array_arg(
    args: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, DispatchToolError> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| DispatchToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("`{key}` must be an array of strings when provided"),
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| DispatchToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: format!("`{key}` entries must be strings"),
                })
        })
        .collect()
}

pub(crate) fn code_arg(args: &JsonObject) -> Result<&str, DispatchToolError> {
    let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
    if code.trim().is_empty() {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "code must not be empty".to_string(),
        });
    }
    if code.len() > MAX_SOURCE_BYTES {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("code exceeds max length {MAX_SOURCE_BYTES} bytes"),
        });
    }
    Ok(code)
}

fn route_scoped_capability_filter(
    args: &JsonObject,
    route_allowed: Option<&BTreeSet<String>>,
) -> Result<ToolScope, DispatchToolError> {
    let requested_upstreams = string_array_arg(args, "upstreams")?;
    if let Some(allowed) = route_allowed
        && requested_upstreams
            .iter()
            .any(|name| !allowed.contains(name))
    {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "route_scope_denied".to_string(),
            message: "Code Mode requested an upstream outside this protected route scope"
                .to_string(),
        });
    }

    let tools = string_array_arg(args, "tools")?;
    let Some(allowed) = route_allowed else {
        return Ok(ToolScope::new(requested_upstreams, tools));
    };
    let filter = if requested_upstreams.is_empty() {
        ToolScope::scoped_namespaces(allowed.iter().cloned().collect(), tools)
    } else {
        ToolScope::scoped_namespaces(requested_upstreams, tools)
    };
    Ok(filter)
}

impl LabMcpServer {
    /// `codemode` gateway tool branch. Self-returns.
    pub(crate) async fn call_tool_codemode_impl(
        &self,
        service: &str,
        args: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let input_tokens = estimate_tokens_args(args);
        let subject = self.request_subject_log_tag(context);
        let actor_key = self.request_actor_key(context);
        let auth = auth_context_from_extensions(&context.extensions);
        if !tool_execute_scope_allowed(auth) {
            let err = DispatchToolError::Forbidden {
                message: "codemode requires one of scopes: lab, lab:admin".to_string(),
                required_scopes: vec!["lab".to_string(), "lab:admin".to_string()],
            };
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = "call_tool",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                kind = "forbidden",
                "gateway codemode denied by scope"
            );
            let env = tool_error_envelope(service, "call_tool", &err);
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                "call_tool",
                "unknown_tool",
                "codemode is not enabled",
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        };
        let config = manager.code_mode_config().await;
        let code = match code_arg(args) {
            Ok(code) => code,
            Err(err) => {
                let env = build_error_extra(
                    service,
                    "call_tool",
                    err.kind(),
                    &err.to_string(),
                    &serde_json::json!({ "param": "code" }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        let capability_filter =
            match route_scoped_capability_filter(args, self.route_scope.allowed_upstreams()) {
                Ok(filter) => filter,
                Err(err) => {
                    let env = tool_error_envelope(service, "call_tool", &err);
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
            };
        let code_hash = hash_arguments(&Value::String(code.to_string()));
        // V4: random component so a crashing/restarting host can never mint a
        // colliding id (mirror runtime.ts:360 `exec_<ts>_<uuid>`).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let execution_id = format!("exec_{now_ms:016}_{}", ulid::Ulid::new());
        let capability_filter_fingerprint = capability_filter.fingerprint();
        // A whole-run pre-confirmation (`confirm: true`) or a resume attempt
        // (`resume_token`) takes the write-free path — pausing is only for a
        // run that has NOT been globally pre-approved.
        let whole_run_confirm = args
            .get("confirm")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_resume_token = args
            .get("resume_token")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty());
        let is_admin_scope =
            auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"));
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            input_tokens,
            "gateway codemode start"
        );
        let decider = manager.code_mode_decider().cloned();

        // ── Resume / reject a paused run (Wave 3) ──
        // A `resume_token` targets an existing paused run. `confirm: false`
        // rejects it; `confirm: true` resumes after full authorization. The
        // block returns early on every failure/reject; reaching its end means a
        // resume was authorized (`resuming = true`, `execution_id = token`).
        let mut execution_id = execution_id;
        // A resume is in progress once the block below reaches its end without
        // an early return.
        let resuming = has_resume_token && whole_run_confirm;
        if has_resume_token {
            let token = args
                .get("resume_token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let Some(decider) = decider.as_ref() else {
                let env = build_error(
                    service,
                    "call_tool",
                    "internal_error",
                    "Code Mode durable pause store is not configured; cannot resume.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            };
            decider.maybe_expire().await;

            if !whole_run_confirm {
                // Reject (Task 3.2). F3: authorize the rejecter BEFORE mutating
                // state — load the run's recorded auth fields, require the
                // integrity check to pass, and require the actor to match the one
                // that started the run (None matches only None, mirroring the
                // resume actor gate). Without this any token holder could
                // force-terminate another actor's run.
                let Some(reject_auth) = decider.run_auth_fields(&token).await else {
                    let env = build_error(
                        service,
                        "call_tool",
                        "unknown_execution",
                        "No Code Mode run for this resume_token; nothing to reject.",
                    );
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                };
                if !reject_auth.verified {
                    let env = build_error(
                        service,
                        "call_tool",
                        "internal_error",
                        "Code Mode run integrity check failed; refusing to reject.",
                    );
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
                let live_actor = actor_key.map(ToOwned::to_owned);
                if reject_auth.actor_key != live_actor {
                    let env = build_error(
                        service,
                        "call_tool",
                        "forbidden",
                        "Only the actor that started a Code Mode run may reject it.",
                    );
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
                // Guarded reject: only a still-`paused` run transitions
                // (`reject_paused`), so this cannot force-terminate a live
                // `running` run.
                match decider
                    .reject_paused(&token, Some("rejected by user"))
                    .await
                {
                    Ok(true) => {
                        tracing::info!(
                            surface = "mcp",
                            service = CODE_MODE_SERVICE,
                            code_mode_tool = %service,
                            action = "reject",
                            subject,
                            actor_key,
                            execution_id = %token,
                            "gateway codemode run rejected by user"
                        );
                        let env = build_error_extra(
                            service,
                            "call_tool",
                            "confirmation_required",
                            "Code Mode run rejected. The pending destructive call was not executed.",
                            &serde_json::json!({ "status": "rejected", "execution_id": token }),
                        );
                        return Ok(CallToolResult::success(vec![Content::text(
                            env.to_string(),
                        )]));
                    }
                    Ok(false) => {
                        let env = build_error(
                            service,
                            "call_tool",
                            "already_resumed",
                            "Code Mode run is not paused (already resumed, rejected, expired, or \
                             unknown); nothing to reject.",
                        );
                        return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                    }
                    Err(err) => {
                        let env = build_error(
                            service,
                            "call_tool",
                            "internal_error",
                            &format!("failed to reject Code Mode run: {err}"),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                    }
                }
            }

            // Resume (Task 3.1) — authorization checks BEFORE the CAS.
            let Some(auth_fields) = decider.run_auth_fields(&token).await else {
                let env = build_error(
                    service,
                    "call_tool",
                    "unknown_execution",
                    "No paused Code Mode run for this resume_token.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            };
            if !auth_fields.verified {
                let env = build_error(
                    service,
                    "call_tool",
                    "internal_error",
                    "Code Mode run integrity check failed; refusing to resume.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            if auth_fields.status != labby_codemode::RunLifecycle::Paused {
                let env = build_error(
                    service,
                    "call_tool",
                    "already_resumed",
                    "Code Mode run is not paused (already resumed, rejected, expired, or \
                     terminal); only a paused run can be resumed.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // Code identity: resubmitted code must hash-match the paused run
            // (source is not persisted — the caller resubmits identical code).
            if auth_fields.code_hash != code_hash {
                let env = build_error(
                    service,
                    "call_tool",
                    "resume_divergence",
                    "Resubmitted Code Mode code does not match the paused run. Resume must \
                     resubmit the identical code.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // Actor identity (V3): the resuming actor must equal the recorded
            // one (None matches only None — never bridges trusted-local↔scoped).
            let live_actor = actor_key.map(ToOwned::to_owned);
            if auth_fields.actor_key != live_actor {
                let env = build_error(
                    service,
                    "call_tool",
                    "forbidden",
                    "Only the actor that started a Code Mode run may resume it.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // Route-scope identity (F2): the run must be resumed under the SAME
            // protected-route scope it paused in. Fail closed BEFORE the CAS so a
            // run paused under route A cannot be resumed under route B even when
            // the caller shares the same actor + capability fingerprint.
            if auth_fields.route_scope != self.route_scope.label() {
                let env = build_error(
                    service,
                    "call_tool",
                    "forbidden",
                    "Code Mode run was paused under a different protected-route scope; \
                     refusing to resume across routes.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // Live authorization (V1 — the critical fix): recompute live caps at
            // resume time. The fingerprint alone is NOT an authz check; the
            // recomputed live capabilities are. If scope narrowed/revoked since
            // pause → fail closed even though the fingerprint matches.
            let live_caps = auth
                .map(|auth| code_mode_capabilities_for_scopes(&auth.scopes))
                .unwrap_or(CodeModeCallerCapabilities {
                    can_execute: true,
                    can_use_snippets: true,
                    is_admin: true,
                });
            let fingerprint_matches =
                auth_fields.capability_filter_fingerprint == capability_filter_fingerprint;
            if !live_caps.can_execute
                || auth_fields.is_admin != live_caps.is_admin
                || !fingerprint_matches
            {
                let env = build_error(
                    service,
                    "call_tool",
                    "forbidden",
                    "Code Mode authorization changed since the run paused; refusing to resume \
                     with narrowed or revoked scope.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // CAS Paused→Running; loser gets already_resumed.
            if !decider.resume_to_running(&token).await {
                let env = build_error(
                    service,
                    "call_tool",
                    "already_resumed",
                    "Code Mode run was concurrently resumed or is no longer paused.",
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
            // Pre-execution intent log (OBSERVABILITY: destructive re-dispatch).
            tracing::info!(
                surface = "mcp",
                service = CODE_MODE_SERVICE,
                code_mode_tool = %service,
                action = "resume.intent",
                subject,
                actor_key,
                execution_id = %token,
                "resuming paused Code Mode run — approved destructive call will re-dispatch"
            );
            execution_id = token;
        }

        // Build the caller now (before deciding pause-capability) so the
        // local-provider exclusion below can consult it. Consumed by
        // `broker.execute` at the end.
        let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
            CodeModeCaller::Scoped {
                capabilities: code_mode_capabilities_for_scopes(&auth.scopes),
                sub: self.request_subject(context).map(ToOwned::to_owned),
            }
        });

        // Local-provider safety (fail closed): Labby's runner-reserved `state`/
        // `git` providers dispatch OFF the decider path
        // (`runner_drive.rs::enqueue_local_provider_call`), so their calls are
        // never journaled and would double-apply on resume. Any run for which
        // local providers are allowed (unscoped admin/trusted-local) must NOT
        // begin a resumable durable run. Excluding it here keeps such runs on the
        // write-free path (no pause, no resume token) — safe by construction.
        let local_providers_allowed =
            labby_codemode::local_providers_allowed(&caller, &capability_filter);

        // Pause-capable path: a decider is injected AND this is a fresh,
        // not-pre-confirmed, non-resume run, AND local providers are not in play.
        // Begin a durable run before driving so destructive calls can journal +
        // pause; else take the write-free path (execution_id stays local, no
        // journaling). A resume never involves local providers (it re-drives a
        // previously non-local run), so `resuming` keeps its pause capability.
        let pause_capable = (decider.is_some()
            && !whole_run_confirm
            && !has_resume_token
            && !local_providers_allowed)
            || resuming;
        if let (true, false, Some(decider)) = (pause_capable, resuming, decider.as_ref()) {
            decider.maybe_expire().await;
            if let Err(err) = decider
                .begin(labby_codemode::BeginRun {
                    execution_id: execution_id.clone(),
                    code_hash: code_hash.clone(),
                    actor_key: actor_key.map(ToOwned::to_owned),
                    is_admin: is_admin_scope,
                    route_scope: self.route_scope.label(),
                    capability_filter_fingerprint: capability_filter_fingerprint.clone(),
                    expires_at_ms: now_ms + pause_ttl_ms(),
                })
                .await
            {
                tracing::warn!(
                    surface = "mcp",
                    service = CODE_MODE_SERVICE,
                    action = "pause.begin",
                    kind = "internal_error",
                    error = %err,
                    "failed to begin durable Code Mode run; falling back to write-free path"
                );
            }
        }
        let broker =
            CodeModeBroker::new(Some(manager.as_ref())).with_execution_id(if pause_capable {
                Some(execution_id.clone())
            } else {
                None
            });
        let before = self.snapshot_catalog().await;
        let mut response = match broker
            .execute(
                code,
                caller,
                self.code_mode_surface(),
                config,
                capability_filter,
            )
            .await
        {
            Ok(response) => {
                let after = self.snapshot_catalog().await;
                self.notify_catalog_changes(&before, &after).await;
                response
            }
            Err(err) => {
                let after = self.snapshot_catalog().await;
                self.notify_catalog_changes(&before, &after).await;
                let calls = err.calls().to_vec();
                let code_mode_calls = code_mode_call_metrics_json(&calls);
                let error_kind = err.kind().to_string();
                let elapsed_ms = started.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = CODE_MODE_SERVICE,
                    code_mode_tool = %service,
                    action = "call_tool",
                    subject,
                    actor_key,
                    actor_label = subject,
                    agent_kind = "agent",
                    code_hash = %code_hash,
                    call_count = calls.len(),
                    code_mode_calls = %code_mode_calls,
                    elapsed_ms,
                    input_tokens,
                    output_tokens = 0,
                    kind = error_kind.as_str(),
                    "gateway codemode failed"
                );
                let tool_error = err.into_tool_error();
                manager
                    .record_code_mode_history(CodeModeHistoryEntry {
                        execution_id: Some(execution_id.clone()),
                        seq: 0,
                        route_scope: self.route_scope.label(),
                        kind: CodeModeHistoryKind::Execute,
                        ok: false,
                        elapsed_ms,
                        input_tokens: Some(input_tokens),
                        output_tokens: Some(0),
                        error_kind: Some(error_kind),
                        calls,
                        match_count: None,
                    })
                    .await;
                let env = tool_error_envelope(service, "call_tool", &tool_error);
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        response.execution_id = Some(execution_id.clone());

        // ── C1 payoff: read the DURABLE status after the pass settles ──
        // A run that swallowed the pause sentinel (Promise.allSettled/try-catch)
        // still completes the sandbox "ok", but the durable status is `paused`.
        // The durable status — NOT the sandbox result — decides the envelope.
        if pause_capable && let Some(decider) = decider.as_ref() {
            match decider.run_status(&execution_id).await {
                labby_codemode::RunLifecycle::Paused => {
                    let pending = decider.list_pending(&execution_id).await;
                    let elapsed_ms = started.elapsed().as_millis();
                    tracing::info!(
                        surface = "mcp",
                        service = CODE_MODE_SERVICE,
                        code_mode_tool = %service,
                        action = "pause",
                        subject,
                        actor_key,
                        execution_id = %execution_id,
                        pending = pending.len(),
                        elapsed_ms,
                        "gateway codemode paused awaiting approval"
                    );
                    return Ok(build_pause_envelope(service, &execution_id, &pending));
                }
                labby_codemode::RunLifecycle::Error => {
                    let message = decider
                        .run_error(&execution_id)
                        .await
                        .unwrap_or_else(|| "Code Mode execution failed".to_string());
                    tracing::warn!(
                        surface = "mcp",
                        service = CODE_MODE_SERVICE,
                        code_mode_tool = %service,
                        action = "call_tool",
                        subject,
                        actor_key,
                        execution_id = %execution_id,
                        kind = "internal_error",
                        elapsed_ms = started.elapsed().as_millis(),
                        "gateway codemode durable error after settle"
                    );
                    let env = build_error(service, "call_tool", "internal_error", &message);
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
                // Running (never paused) or already terminal-ok — mark completed
                // and fall through to the normal completed envelope. F5: a
                // swallowed failure here leaves the run non-terminal (it lingers
                // until TTL expiry). Not a correctness bug for the response, but
                // log it so the leak is observable.
                _ => match decider
                    .set_status(&execution_id, labby_codemode::RunLifecycle::Completed, None)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(
                            surface = "mcp",
                            service = CODE_MODE_SERVICE,
                            action = "complete",
                            kind = "internal_error",
                            execution_id = %execution_id,
                            "could not mark Code Mode run completed (no row updated); \
                             run will linger until TTL expiry"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            surface = "mcp",
                            service = CODE_MODE_SERVICE,
                            action = "complete",
                            kind = "internal_error",
                            execution_id = %execution_id,
                            error = %err,
                            "failed to mark Code Mode run completed; run will linger \
                             until TTL expiry"
                        );
                    }
                },
            }
        }

        // Mirror the upstream's `_meta.ui` verbatim onto the codemode result so
        // the host renders the native mcp-ui widget (last-wins). The widget
        // itself is driven by the `ui://` resource read, not by inline content,
        // so the Code Mode trace content is left intact.
        let ui_meta = response.ui.as_ref().map(|ui| {
            let mut map = serde_json::Map::new();
            map.insert("ui".to_string(), ui.ui_meta.clone());
            Meta(map)
        });
        let mirrored_resource_uri = response.ui.as_ref().and_then(|ui| {
            ui.ui_meta
                .get("resourceUri")
                .and_then(|value| value.as_str())
        });
        if response.ui.is_some() {
            tracing::info!(
                surface = "mcp",
                service = CODE_MODE_SERVICE,
                code_mode_tool = %service,
                action = "mcp_app.mirror",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                resource_uri = mirrored_resource_uri.unwrap_or("<unknown>"),
                "mirroring upstream MCP App widget metadata onto codemode result"
            );
        }
        let output = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        let output_tokens = estimate_tokens(&output);
        manager
            .record_code_mode_history(CodeModeHistoryEntry {
                execution_id: Some(execution_id.clone()),
                seq: 0,
                route_scope: self.route_scope.label(),
                kind: CodeModeHistoryKind::Execute,
                ok: true,
                elapsed_ms: started.elapsed().as_millis(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                error_kind: None,
                calls: response.calls.clone(),
                match_count: None,
            })
            .await;
        let is_admin = auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"));
        if is_admin && code.len() <= MAX_SOURCE_BYTES {
            manager
                .record_code_mode_source(CodeModeExecutionSource {
                    execution_id: execution_id.clone(),
                    created_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as i64)
                        .unwrap_or_default(),
                    actor_key: actor_key.map(ToOwned::to_owned),
                    is_admin,
                    route_scope: self.route_scope.label(),
                    surface: self.code_mode_surface(),
                    capability_filter_fingerprint,
                    code: code.to_string(),
                })
                .await;
        }
        let mut structured = code_mode_execute_trace(&response);
        if let Some(object) = structured.as_object_mut() {
            object.insert(
                "execution_id".to_string(),
                Value::String(execution_id.clone()),
            );
            object.insert("input_tokens".to_string(), Value::from(input_tokens as u64));
            object.insert(
                "output_tokens".to_string(),
                Value::from(output_tokens as u64),
            );
        }
        let trace_result_type = structured
            .get("result_shape")
            .and_then(|shape| shape.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let trace_has_result = structured.get("result").is_some();
        let shape_truncated = response
            .result_shaping
            .as_ref()
            .map(|shape| shape.truncated)
            .unwrap_or(false);
        let legacy_truncated = response
            .result
            .as_ref()
            .and_then(|result| result.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let truncated = shape_truncated || legacy_truncated;
        let result_shape_policy = response
            .result_shaping
            .as_ref()
            .and_then(|shape| serde_json::to_value(shape.policy).ok())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "legacy".to_string());
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            call_count = response.calls.len(),
            code_mode_calls = %code_mode_call_metrics_json(&response.calls),
            artifact_writes = response.artifacts.len(),
            truncated,
            result_shape_policy,
            elapsed_ms = started.elapsed().as_millis(),
            input_tokens,
            output_tokens,
            trace_has_result,
            trace_result_type,
            mirrored_ui_resource_uri = mirrored_resource_uri.unwrap_or("<none>"),
            "gateway codemode ok"
        );
        Ok(call_result_with_structured(output, structured, ui_meta))
    }
}

/// Configured pause TTL in ms (`LAB_CODE_MODE_PAUSE_TTL_MS`, default 24h).
fn pause_ttl_ms() -> i64 {
    std::env::var("LAB_CODE_MODE_PAUSE_TTL_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(24 * 60 * 60 * 1000)
}

/// Build the paused envelope: a `confirmation_required` error carrying the
/// `resume_token` (the execution id) plus the pending-call summary (port of
/// `proxy-tool.ts:108-112` `{ status: "paused", pending }`). The model resumes
/// with `resume_token` + `confirm: true` + the identical resubmitted code, or
/// rejects with `confirm: false`.
fn build_pause_envelope(
    service: &str,
    execution_id: &str,
    pending: &[labby_codemode::PendingCall],
) -> CallToolResult {
    let pending_json: Vec<Value> = pending
        .iter()
        .map(|p| serde_json::json!({ "seq": p.seq, "tool_id": p.tool_id }))
        .collect();
    let env = build_error_extra(
        service,
        "call_tool",
        "confirmation_required",
        "Code Mode paused awaiting human approval of a destructive tool call. \
         Tell the user what is pending and wait — do NOT re-issue the code. To \
         continue, resubmit the identical code with `resume_token` and \
         `confirm: true`; to cancel, use `confirm: false`.",
        &serde_json::json!({
            "status": "paused",
            "execution_id": execution_id,
            "resume_token": execution_id,
            "pending": pending_json,
        }),
    );
    CallToolResult::error(vec![Content::text(env.to_string())])
}

fn code_mode_capabilities_for_scopes(scopes: &[String]) -> CodeModeCallerCapabilities {
    let is_admin = scopes.iter().any(|scope| scope == "lab:admin");
    CodeModeCallerCapabilities {
        can_execute: scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")),
        can_use_snippets: is_admin,
        is_admin,
    }
}

fn call_result_with_structured(
    text: String,
    structured: Value,
    ui_meta: Option<Meta>,
) -> CallToolResult {
    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(structured);
    result.meta = ui_meta;
    result
}

#[cfg(test)]
mod tests;
