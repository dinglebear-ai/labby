//! Sandbox bridge emission and promise settlement.

use super::*;

pub(super) fn wrap_code_mode(code: &str, proxy: &str) -> String {
    // The execute wrapper body (assign → typeof check → invoke) has a single
    // source in `code_mode_main_invoker` so the contract cannot diverge.
    // It is interpolated as a named arg (`{invoker}`) so its literal JS braces
    // are substituted verbatim and need no `{{`/`}}` escaping.
    let invoker = code_mode_main_invoker(code);
    format!(
        r#"
globalThis.__labPendingToolCalls = new Map();
globalThis.__labSnippetStack = [];
globalThis.__labSnippetResolveCount = 0;
globalThis.__labSnippetResolvedBytes = 0;
globalThis.__labSnippetMaxDepth = 8;
globalThis.__labSnippetMaxResolves = 32;
globalThis.__labSnippetMaxBytes = {snippet_max_bytes};
{codec}
globalThis.callTool = (id, params = {{}}) => {{
  if (typeof id !== "string" || id.trim() === "") {{
    throw new TypeError("callTool id must be a non-empty string");
  }}
  if (params === null || typeof params !== "object" || Array.isArray(params)) {{
    throw new TypeError("callTool params must be a JSON object");
  }}
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitToolCall(id, __labEncodeResult(params));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "tool", resolve, reject }});
  }});
}};
globalThis.writeArtifact = (path, content, options = {{}}) => {{
  if (typeof path !== "string" || path.trim() === "") {{
    throw new TypeError("writeArtifact path must be a non-empty string");
  }}
  if (typeof content !== "string") {{
    throw new TypeError("writeArtifact content must be a string");
  }}
  if (options === null || typeof options !== "object" || Array.isArray(options)) {{
    throw new TypeError("writeArtifact options must be a JSON object");
  }}
  if (options.contentType !== undefined && typeof options.contentType !== "string") {{
    throw new TypeError("writeArtifact options.contentType must be a string");
  }}
  const contentType = options.contentType ?? null;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitArtifactWrite(path, content, contentType);
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "artifact", resolve, reject }});
  }});
}};
globalThis.__labRunSnippet = (name, input = {{}}) => {{
  if (typeof name !== "string" || name.trim() === "") {{
    return Promise.reject(new Error(JSON.stringify({{kind: "bad_snippet_name", message: "codemode.run name must be a non-empty string"}})));
  }}
  if (input === null || typeof input !== "object" || Array.isArray(input)) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "invalid_param", message: "codemode.run input must be a JSON object"}})));
  }}
  if (globalThis.__labSnippetStack.indexOf(name) !== -1) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_recursion_limit", message: "snippet recursion detected for `" + name + "`"}})));
  }}
  if (globalThis.__labSnippetStack.length >= globalThis.__labSnippetMaxDepth) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_depth_exceeded", message: "snippet depth limit exceeded"}})));
  }}
  if (globalThis.__labSnippetResolveCount >= globalThis.__labSnippetMaxResolves) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_resolve_limit", message: "snippet resolve limit exceeded"}})));
  }}
  globalThis.__labSnippetResolveCount++;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitSnippetResolve(name, __labEncodeResult(input));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "snippet", name, resolve, reject }});
  }});
}};
globalThis.__labCodemodeStep = (name, fn) => {{
  if (typeof fn !== "function") {{
    return Promise.reject(new Error(JSON.stringify({{kind: "invalid_param", message: "codemode.step requires a function"}})));
  }}
  if (typeof name !== "string" || name.trim() === "") {{
    return Promise.reject(new Error(JSON.stringify({{kind: "invalid_param", message: "codemode.step name must be a non-empty string"}})));
  }}
  // Phase 1: ask the host to decide replay-vs-execute BEFORE running fn.
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitStepBegin(name);
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "step_begin", name, fn, resolve, reject, __stepBeginSeq: seq }});
  }});
}};
globalThis.__labSettlePendingOperation = (message) => {{
  const input = JSON.parse(message);
  const pending = globalThis.__labPendingToolCalls.get(input.seq);
  if (!pending) {{
    throw new Error("runner received a response for an unknown pending operation");
  }}
  globalThis.__labPendingToolCalls.delete(input.seq);
  if (input.type === "step_decision") {{
    if (pending.kind !== "step_begin") {{
      throw new Error("runner received a step decision for a non-step operation");
    }}
    // Reserved future replay response: return the supplied value WITHOUT
    // running fn. The current host never sends this branch.
    if (Object.prototype.hasOwnProperty.call(input, "replay") && input.replay !== null && input.replay !== undefined) {{
      pending.resolve(__labDecodeResult(input.replay));
      return;
    }}
    // Current path: run fn, then offer its result for journal recording via a
    // StepResult round-trip.
    const beginSeq = pending.__stepBeginSeq;
    Promise.resolve().then(pending.fn).then((value) => {{
      const encoded = __labEncodeResult(value === undefined ? null : value);
      return new Promise((resolve, reject) => {{
        // StepResult reuses the StepBegin seq (the host records at that journal
        // entry). Key the ack on the same seq — the begin entry was already
        // deleted above, so there is no collision.
        globalThis.__labEmitStepResult(beginSeq, encoded);
        globalThis.__labPendingToolCalls.set(beginSeq, {{ kind: "step_result", value: value, resolve, reject }});
      }});
    }}).then(pending.resolve, pending.reject);
    return;
  }}
  if (input.type === "step_recorded") {{
    if (pending.kind !== "step_result") {{
      throw new Error("runner received a step recorded ack for a non-step-result operation");
    }}
    pending.resolve(pending.value);
    return;
  }}
  if (input.type === "tool_result") {{
    pending.resolve(__labDecodeResult(input.result));
    return;
  }}
  if (input.type === "snippet_resolved") {{
    if (pending.kind !== "snippet") {{
      throw new Error("runner received snippet code for a non-snippet operation");
    }}
    if (typeof input.code !== "string") {{
      pending.reject(new Error(JSON.stringify({{kind: "invalid_snippet_resolution", message: "resolved snippet code must be a string"}})));
      return;
    }}
    globalThis.__labSnippetResolvedBytes += input.code.length;
    if (globalThis.__labSnippetResolvedBytes > globalThis.__labSnippetMaxBytes) {{
      pending.reject(new Error(JSON.stringify({{kind: "snippet_budget_exceeded", message: "resolved snippet code budget exceeded"}})));
      return;
    }}
    Promise.resolve().then(async () => {{
      globalThis.__labSnippetStack.push(pending.name);
      try {{
        return await (eval("(" + input.code + ")"))(__labDecodeResult(input.input));
      }} finally {{
        globalThis.__labSnippetStack.pop();
      }}
    }}).then(pending.resolve, pending.reject);
    return;
  }}
  if (input.type === "tool_error") {{
    // Reject with the complete model-facing error contract as pure JSON. Keep
    // transport-only fields (type/seq) out of Error.message so caught and
    // uncaught failures expose the same stable object to caller JavaScript.
    const error = {{}};
    for (const key of Object.keys(input)) {{
      if (key !== "type" && key !== "seq") error[key] = input[key];
    }}
    pending.reject(new Error(JSON.stringify(error)));
    return;
  }}
  throw new Error("runner received unexpected protocol message");
}};
globalThis.__labSettleToolCall = globalThis.__labSettlePendingOperation;
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})();
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
        snippet_max_bytes = MAX_SNIPPET_RESOLVED_BYTES_PER_RUN,
    )
}

pub(super) enum JavyMainPromiseState {
    Pending,
    /// The async function returned. `result` is the JSON-serialized return value,
    /// or None when the function returned undefined.
    Resolved(Option<Value>),
    Rejected(String),
}

pub(super) fn javy_emit_tool_call(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let id_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "callTool id must be a non-empty string"))?;
    let id = javy::val_to_string(&cx, id_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if id.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "callTool id must be a non-empty string",
        ));
    }

    if let Err(error) = crate::protocol::check_tool_id_bytes(&id) {
        let message = serde_json::to_string(&error)
            .map_err(|error| javy_type_error(cx.clone(), error.to_string()))?;
        return Err(javy_type_error(cx.clone(), message));
    }

    let params_json = args
        .0
        .get(1)
        .map(|params| cx.json_stringify(params.clone()))
        .transpose()?
        .flatten()
        .map(|params| params.to_string())
        .transpose()?
        .unwrap_or_else(|| "{}".to_string());
    let params: Value = serde_json::from_str(&params_json).map_err(|err| {
        javy_type_error(
            cx.clone(),
            format!("callTool params must be JSON-serializable: {err}"),
        )
    })?;
    if !params.is_object() {
        return Err(javy_type_error(
            cx.clone(),
            "callTool params must be a JSON object",
        ));
    }

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::ToolCall { seq, id, params })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

pub(super) fn javy_emit_artifact_write(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let path_value = args.0.first().ok_or_else(|| {
        javy_type_error(cx.clone(), "writeArtifact path must be a non-empty string")
    })?;
    let path = javy::val_to_string(&cx, path_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if path.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "writeArtifact path must be a non-empty string",
        ));
    }

    let content_value = args
        .0
        .get(1)
        .ok_or_else(|| javy_type_error(cx.clone(), "writeArtifact content must be a string"))?;
    let content = javy::val_to_string(&cx, content_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;

    let content_type = args
        .0
        .get(2)
        .filter(|value| !value.is_null() && !value.is_undefined())
        .map(|value| javy::val_to_string(&cx, value.clone()))
        .transpose()
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::ArtifactWrite {
        seq,
        path,
        content,
        content_type,
    })
    .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

pub(super) fn javy_emit_snippet_resolve(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let name_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "snippet name must be a non-empty string"))?;
    let name = javy::val_to_string(&cx, name_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if name.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "snippet name must be a non-empty string",
        ));
    }

    let input_json = args
        .0
        .get(1)
        .map(|input| cx.json_stringify(input.clone()))
        .transpose()?
        .flatten()
        .map(|input| input.to_string())
        .transpose()?
        .unwrap_or_else(|| "{}".to_string());
    let input: Value = serde_json::from_str(&input_json).map_err(|err| {
        javy_type_error(
            cx.clone(),
            format!("snippet input must be JSON-serializable: {err}"),
        )
    })?;
    if !input.is_object() {
        return Err(javy_type_error(
            cx.clone(),
            "snippet input must be a JSON object",
        ));
    }

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::SnippetResolve { seq, name, input })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

pub(super) fn javy_emit_step_begin(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let name_value = args.0.first().ok_or_else(|| {
        javy_type_error(cx.clone(), "codemode.step name must be a non-empty string")
    })?;
    let name = javy::val_to_string(&cx, name_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if name.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "codemode.step name must be a non-empty string",
        ));
    }

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::StepBegin { seq, name })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

pub(super) fn javy_emit_step_result(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    // The StepResult targets the SAME seq the StepBegin was journaled under — the
    // host records the step value at that log-entry seq. This is NOT a fresh seq
    // allocation (unlike tool calls / step begins): recording the result must land
    // on the already-journaled step entry, so the caller passes the begin seq back.
    let seq_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "codemode.step result requires a seq"))?;
    let seq_f = seq_value
        .as_number()
        .ok_or_else(|| javy_type_error(cx.clone(), "codemode.step result seq must be a number"))?;
    // seq originates from `next_runner_seq` (a u64 cast to a JS number), so it is
    // a non-negative integer well within the f64-exact range.
    let seq = seq_f as u64;

    let value_json = args
        .0
        .get(1)
        .map(|value| cx.json_stringify(value.clone()))
        .transpose()?
        .flatten()
        .map(|value| value.to_string())
        .transpose()?
        .unwrap_or_else(|| "null".to_string());
    let value: Value = serde_json::from_str(&value_json).map_err(|err| {
        javy_type_error(
            cx.clone(),
            format!("codemode.step result must be JSON-serializable: {err}"),
        )
    })?;

    runner_emit(CodeModeRunnerOutput::StepResult { seq, value })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

pub(super) fn javy_settle_tool_promise(
    runtime: &javy::Runtime,
    input: &CodeModeRunnerInput,
) -> Result<(), String> {
    let message = serde_json::to_string(input).map_err(|err| err.to_string())?;
    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<()> {
            let settle: javy::quickjs::Function<'_> =
                cx.globals().get("__labSettlePendingOperation")?;
            settle.call::<_, ()>((message,))?;
            Ok(())
        })
        .map_err(javy_error_message)?;
    runtime
        .resolve_pending_jobs()
        .map_err(|err| err.to_string())
}

pub(super) fn javy_main_promise_state(
    runtime: &javy::Runtime,
) -> Result<JavyMainPromiseState, String> {
    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<JavyMainPromiseState> {
            let promise: javy::quickjs::Promise<'_> = cx.globals().get("__labMainPromise")?;
            match promise.result::<javy::quickjs::Value<'_>>() {
                None => Ok(JavyMainPromiseState::Pending),
                Some(Ok(val)) => {
                    // Serialize the resolved value to JSON via cx.json_stringify.
                    // undefined cannot be stringified and maps to None (no result).
                    // null is a real JSON value and must round-trip as Some(Null).
                    let result = if val.is_undefined() {
                        None
                    } else {
                        match cx.json_stringify(val) {
                            Ok(Some(json_str)) => {
                                let json_text = json_str.to_string()?;
                                let value: Value = match serde_json::from_str(&json_text) {
                                    Ok(value) => value,
                                    Err(err) => {
                                        return Ok(JavyMainPromiseState::Rejected(format!(
                                            "Code Mode result must be JSON-serializable: {err}"
                                        )));
                                    }
                                };
                                Some(value)
                            }
                            Ok(None) => {
                                return Ok(JavyMainPromiseState::Rejected(
                                    "Code Mode result must be JSON-serializable".to_string(),
                                ));
                            }
                            Err(err) => {
                                return Ok(JavyMainPromiseState::Rejected(format!(
                                    "Code Mode result must be JSON-serializable: {}",
                                    javy::from_js_error(cx.clone(), err)
                                )));
                            }
                        }
                    };
                    Ok(JavyMainPromiseState::Resolved(result))
                }
                Some(Err(err)) => {
                    // Capture a debug representation before `from_js_error`
                    // consumes `err`; used as a fallback when the stringified
                    // JS error is empty (lab-4uele).
                    let debug_fallback = format!("{err:?}");
                    let message = javy::from_js_error(cx.clone(), err).to_string();
                    let message = if message.is_empty() {
                        debug_fallback
                    } else {
                        message
                    };
                    Ok(JavyMainPromiseState::Rejected(message))
                }
            }
        })
        .map_err(javy_error_message)
}

pub(super) fn javy_type_error(
    message_context: javy::quickjs::Ctx<'_>,
    message: impl Into<String>,
) -> javy::quickjs::Error {
    javy::to_js_error(message_context, anyhow::anyhow!(message.into()))
}

pub(super) fn next_runner_seq(cx: &javy::quickjs::Ctx<'_>) -> javy::quickjs::Result<u64> {
    RUNNER_STATE
        .with(|state| {
            let mut state = state.borrow_mut();
            let state = state
                .as_mut()
                .ok_or_else(|| "runner state is not initialized".to_string())?;
            let seq = state.next_seq;
            state.next_seq = state.next_seq.saturating_add(1);
            Ok::<_, String>(seq)
        })
        .map_err(|err| javy_type_error(cx.clone(), err))
}

pub(super) fn javy_error_message(error: javy::quickjs::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod wrapper_tests;

#[cfg(test)]
mod id_tests;
