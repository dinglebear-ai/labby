//! Javy dynamic-linking code generation for one Code Mode execution.

use anyhow::{Context, Result};
use javy_codegen::{Generator, JS, LinkingKind, SourceEmbedding};

use crate::wrapper::{CODE_MODE_VALUE_CODEC_JS, code_mode_main_invoker};

pub(crate) fn wrap_code_mode_for_wasm(code: &str, proxy: &str) -> String {
    let invoker = code_mode_main_invoker(code);
    format!(
        r#"
globalThis.__labPendingToolCalls = new Map();
globalThis.__labSnippetStack = [];
globalThis.__labSnippetResolveCount = 0;
globalThis.__labSnippetResolvedBytes = 0;
globalThis.__labSnippetMaxDepth = 8;
globalThis.__labSnippetMaxResolves = 32;
globalThis.__labSnippetMaxBytes = 262144;
{codec}
globalThis.__labEncodeJsonValue = (label, value) => {{
  try {{
    const encoded = __labEncodeResult(value);
    const text = JSON.stringify(encoded);
    if (text === undefined) throw new TypeError(label + " must be JSON-serializable");
    return JSON.parse(text);
  }} catch (error) {{
    const message = String(error && error.message || error);
    if (message.indexOf("JSON-serializable") !== -1) throw error;
    throw new TypeError(label + " must be JSON-serializable: " + message);
  }}
}};
globalThis.callTool = (id, params = {{}}) => {{
  if (typeof id !== "string" || id.trim() === "") throw new TypeError("callTool id must be a non-empty string");
  if (params === null || typeof params !== "object" || Array.isArray(params)) throw new TypeError("callTool params must be a JSON object");
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitToolCall(JSON.stringify({{ id, params: __labEncodeJsonValue("callTool params", params) }}));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "tool", resolve, reject }});
  }});
}};
globalThis.writeArtifact = (path, content, options = {{}}) => {{
  if (typeof path !== "string" || path.trim() === "") throw new TypeError("writeArtifact path must be a non-empty string");
  if (typeof content !== "string") throw new TypeError("writeArtifact content must be a string");
  if (options === null || typeof options !== "object" || Array.isArray(options)) throw new TypeError("writeArtifact options must be a JSON object");
  if (options.contentType !== undefined && typeof options.contentType !== "string") throw new TypeError("writeArtifact options.contentType must be a string");
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitArtifactWrite(JSON.stringify({{ path, content, content_type: options.contentType ?? null }}));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "artifact", resolve, reject }});
  }});
}};
globalThis.__labRunSnippet = (name, input = {{}}) => {{
  if (typeof name !== "string" || name.trim() === "") return Promise.reject(new Error(JSON.stringify({{kind: "bad_snippet_name", message: "codemode.run name must be a non-empty string"}})));
  if (input === null || typeof input !== "object" || Array.isArray(input)) return Promise.reject(new Error(JSON.stringify({{kind: "invalid_param", message: "codemode.run input must be a JSON object"}})));
  if (globalThis.__labSnippetStack.indexOf(name) !== -1) return Promise.reject(new Error(JSON.stringify({{kind: "snippet_recursion_limit", message: "snippet recursion detected for `" + name + "`"}})));
  if (globalThis.__labSnippetStack.length >= globalThis.__labSnippetMaxDepth) return Promise.reject(new Error(JSON.stringify({{kind: "snippet_depth_exceeded", message: "snippet depth limit exceeded"}})));
  if (globalThis.__labSnippetResolveCount >= globalThis.__labSnippetMaxResolves) return Promise.reject(new Error(JSON.stringify({{kind: "snippet_resolve_limit", message: "snippet resolve limit exceeded"}})));
  globalThis.__labSnippetResolveCount++;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitSnippetResolve(JSON.stringify({{ name, input: __labEncodeJsonValue("codemode.run input", input) }}));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "snippet", name, resolve, reject }});
  }});
}};
globalThis.__labSettlePendingOperation = (message) => {{
  const input = JSON.parse(message);
  const pending = globalThis.__labPendingToolCalls.get(input.seq);
  if (!pending) throw new Error("runner received a response for an unknown pending operation");
  globalThis.__labPendingToolCalls.delete(input.seq);
  if (input.type === "tool_result") {{
    pending.resolve(__labDecodeResult(input.result));
    return;
  }}
  if (input.type === "snippet_resolved") {{
    if (pending.kind !== "snippet") throw new Error("runner received snippet code for a non-snippet operation");
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
    pending.reject(new Error(JSON.stringify({{kind: input.kind, message: input.message}})));
    return;
  }}
  throw new Error("runner received unexpected protocol message");
}};
globalThis.__labSettleToolCall = globalThis.__labSettlePendingOperation;
globalThis.__labEmitSuccess = (value) => {{
  try {{
    if (value === undefined) {{
      globalThis.__labEmitDone(JSON.stringify({{ has_result: false }}));
      return;
    }}
    globalThis.__labEmitDone(JSON.stringify({{ result: __labEncodeJsonValue("Code Mode result", value), has_result: true }}));
  }} catch (error) {{
    const raw = String(error && error.message || error);
    const message = raw.indexOf("Code Mode result must be JSON-serializable") !== -1 ? raw : "Code Mode result must be JSON-serializable: " + raw;
    globalThis.__labEmitDone(JSON.stringify({{ error: message }}));
  }}
}};
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})().then(
  (value) => globalThis.__labEmitSuccess(value),
  (error) => globalThis.__labEmitDone(JSON.stringify({{ error: String(error && error.message || error) }}))
);
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
    )
}

#[cfg(test)]
pub(crate) async fn compile_code_mode_wasm(
    plugin: &javy_codegen::Plugin,
    code: &str,
    proxy: &str,
) -> Result<Vec<u8>> {
    let source = wrap_code_mode_for_wasm(code, proxy);
    compile_wrapped_code_mode_wasm(plugin, &source).await
}

pub(crate) async fn compile_wrapped_code_mode_wasm(
    plugin: &javy_codegen::Plugin,
    source: &str,
) -> Result<Vec<u8>> {
    let js = JS::from_string(source.to_string());
    let mut generator = Generator::new(plugin.clone());
    generator
        .linking(LinkingKind::Dynamic)
        .source_embedding(SourceEmbedding::Omitted)
        .producer_version(format!("labby-codemode-{}", env!("CARGO_PKG_VERSION")))
        .deterministic(true);
    generator
        .generate(&js)
        .await
        .context("failed to generate Code Mode Wasm module")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_preserves_core_globals() {
        let wrapped = wrap_code_mode_for_wasm("async () => 42", "var codemode = {};");
        assert!(wrapped.contains("globalThis.callTool"));
        assert!(wrapped.contains("globalThis.writeArtifact"));
        assert!(wrapped.contains("globalThis.__labRunSnippet"));
        assert!(wrapped.contains("globalThis.__labMainPromise"));
        assert!(wrapped.contains("__labEmitDone"));
        assert!(wrapped.contains("globalThis.__labEmitSuccess"));
    }

    #[test]
    fn wrapper_preserves_pending_promise_fanout_bridge() {
        let wrapped = wrap_code_mode_for_wasm(
            "async () => Promise.all([callTool('a::b', {}), writeArtifact('x', 'y')])",
            "var codemode = {};",
        );
        assert!(wrapped.contains("globalThis.__labPendingToolCalls = new Map()"));
        assert!(wrapped.contains("new Promise((resolve, reject)"));
        assert!(wrapped.contains("__labEmitToolCall(JSON.stringify"));
        assert!(wrapped.contains("__labEmitArtifactWrite(JSON.stringify"));
        assert!(wrapped.contains("__labEmitSnippetResolve(JSON.stringify"));
        assert!(wrapped.contains("globalThis.__labSettlePendingOperation"));
        assert!(wrapped.contains("pending.resolve(__labDecodeResult(input.result))"));
        assert!(wrapped.contains("pending.reject(new Error(JSON.stringify"));
    }

    #[test]
    fn wrapper_reports_non_json_serializable_results() {
        let wrapped = wrap_code_mode_for_wasm("async () => 1n", "");
        assert!(wrapped.contains("Code Mode result must be JSON-serializable"));
        assert!(wrapped.contains("(value) => globalThis.__labEmitSuccess(value)"));
    }

    #[test]
    fn wrapper_does_not_use_stale_result_pointer_bridge() {
        let wrapped = wrap_code_mode_for_wasm("async () => 42", "");
        assert!(!wrapped.contains(concat!("__lab", "_result_ptr")));
        assert!(!wrapped.contains(concat!("__lab", "_result_len")));
        assert!(!wrapped.contains(concat!("lab", "_bridge")));
    }

    #[tokio::test]
    async fn generated_module_compiles_under_shared_engine() {
        let plugin = crate::wasm_plugin::load_wasm_plugin().unwrap();
        let wasm = compile_code_mode_wasm(&plugin.plugin, "async () => 2 + 2", "")
            .await
            .unwrap();
        wasmtime::Module::from_binary(&plugin.engine, &wasm).unwrap();
    }
}
