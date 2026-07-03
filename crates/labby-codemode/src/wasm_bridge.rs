//! Wasmtime imports that preserve the existing parent-owned runner protocol.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use wasmtime::{AsContext, AsContextMut, Caller, Linker, Memory, Store, TypedFunc};
use wasmtime_wasi::p1::WasiP1Ctx;

use crate::error::ToolError;
use crate::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput, CodeModeRunnerResult};
use crate::runner_io::{runner_emit_blocking, runner_next_seq_blocking};

pub(crate) const CODE_MODE_WASM_BRIDGE_MAX_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct WasmRunState {
    pub(crate) wasi: WasiP1Ctx,
    pub(crate) memory: Option<Memory>,
    pub(crate) compile_src: Option<TypedFunc<(i32, i32), i32>>,
    pub(crate) invoke: Option<TypedFunc<(i32, i32, i32, i32, i32), ()>>,
    pub(crate) cabi_realloc: Option<TypedFunc<(i32, i32, i32, i32), i32>>,
    pub(crate) settlement_bytecode: Option<(i32, i32)>,
    pub(crate) pending_settlement_input: Option<String>,
    pub(crate) done: Option<Result<CodeModeRunnerResult, ToolError>>,
    pub(crate) emitted_operations: u64,
    pub(crate) bridge_roundtrip_ms: u128,
    pub(crate) limiter: crate::wasm_runner::CodeModeLimiter,
}

impl Default for WasmRunState {
    fn default() -> Self {
        let mut builder = wasmtime_wasi::WasiCtxBuilder::new();
        deterministic_wasi_ctx::add_determinism_to_wasi_ctx_builder(&mut builder);
        Self {
            wasi: builder.build_p1(),
            memory: None,
            compile_src: None,
            invoke: None,
            cabi_realloc: None,
            settlement_bytecode: None,
            pending_settlement_input: None,
            done: None,
            emitted_operations: 0,
            bridge_roundtrip_ms: 0,
            limiter: crate::wasm_runner::CodeModeLimiter,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ToolCallPayload {
    id: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
struct ArtifactWritePayload {
    path: String,
    content: String,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SnippetResolvePayload {
    name: String,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct DonePayload {
    #[serde(default)]
    has_result: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) fn checked_guest_bytes<'a, T: 'static>(
    store: impl Into<wasmtime::StoreContext<'a, T>>,
    memory: &Memory,
    ptr: i32,
    len: i32,
    cap: usize,
) -> Result<Vec<u8>, ToolError> {
    if ptr < 0 || len < 0 {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "Code Mode Wasm bridge received a negative pointer or length".to_string(),
        });
    }
    let ptr = ptr as usize;
    let len = len as usize;
    if len > cap {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("Code Mode Wasm bridge payload exceeded {cap} bytes"),
        });
    }
    let end = ptr.checked_add(len).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "Code Mode Wasm bridge pointer overflowed".to_string(),
    })?;
    let data = memory.data(store);
    let slice = data.get(ptr..end).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "Code Mode Wasm bridge pointer was outside guest memory".to_string(),
    })?;
    Ok(slice.to_vec())
}

pub(crate) fn read_guest_string<'a, T: 'static>(
    store: impl Into<wasmtime::StoreContext<'a, T>>,
    memory: &Memory,
    ptr: i32,
    len: i32,
    cap: usize,
) -> Result<String, ToolError> {
    let bytes = checked_guest_bytes(store, memory, ptr, len, cap)?;
    String::from_utf8(bytes).map_err(|err| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("Code Mode Wasm bridge payload was not UTF-8: {err}"),
    })
}

pub(crate) fn read_guest_json<'a, S: 'static, T: DeserializeOwned>(
    store: impl Into<wasmtime::StoreContext<'a, S>>,
    memory: &Memory,
    ptr: i32,
    len: i32,
    cap: usize,
) -> Result<T, ToolError> {
    let text = read_guest_string(store, memory, ptr, len, cap)?;
    serde_json::from_str(&text).map_err(|err| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("Code Mode Wasm bridge payload was not valid JSON: {err}"),
    })
}

pub(crate) fn install_lab_imports(
    linker: &mut Linker<WasmRunState>,
    namespace: &str,
) -> anyhow::Result<()> {
    linker.func_wrap(namespace, "lab_emit_tool_call", lab_emit_tool_call)?;
    linker.func_wrap(
        namespace,
        "lab_emit_artifact_write",
        lab_emit_artifact_write,
    )?;
    linker.func_wrap(
        namespace,
        "lab_emit_snippet_resolve",
        lab_emit_snippet_resolve,
    )?;
    linker.func_wrap(namespace, "lab_emit_done", lab_emit_done)?;
    linker.func_wrap(namespace, "lab_pending_input_len", lab_pending_input_len)?;
    linker.func_wrap(namespace, "lab_pending_input_copy", lab_pending_input_copy)?;
    linker.func_wrap(namespace, "lab_console_log", lab_console_log)?;
    Ok(())
}

fn lab_emit_tool_call(
    caller: Caller<'_, WasmRunState>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<i32> {
    emit_with_payload(caller, ptr, len, |payload: ToolCallPayload, seq| {
        if payload.id.trim().is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "callTool id must be a non-empty string".to_string(),
            });
        }
        if !payload.params.is_object() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "callTool params must be a JSON object".to_string(),
            });
        }
        Ok(CodeModeRunnerOutput::ToolCall {
            seq,
            id: payload.id,
            params: payload.params,
        })
    })
}

fn lab_emit_artifact_write(
    caller: Caller<'_, WasmRunState>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<i32> {
    emit_with_payload(caller, ptr, len, |payload: ArtifactWritePayload, seq| {
        Ok(CodeModeRunnerOutput::ArtifactWrite {
            seq,
            path: payload.path,
            content: payload.content,
            content_type: payload.content_type,
        })
    })
}

fn lab_emit_snippet_resolve(
    caller: Caller<'_, WasmRunState>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<i32> {
    emit_with_payload(caller, ptr, len, |payload: SnippetResolvePayload, seq| {
        if payload.name.trim().is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "snippet name must be a non-empty string".to_string(),
            });
        }
        if !payload.input.is_object() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "snippet input must be a JSON object".to_string(),
            });
        }
        Ok(CodeModeRunnerOutput::SnippetResolve {
            seq,
            name: payload.name,
            input: payload.input,
        })
    })
}

fn emit_with_payload<T, F>(
    mut caller: Caller<'_, WasmRunState>,
    ptr: i32,
    len: i32,
    build: F,
) -> wasmtime::Result<i32>
where
    T: DeserializeOwned,
    F: FnOnce(T, u64) -> Result<CodeModeRunnerOutput, ToolError>,
{
    let Some(memory) = caller.data().memory else {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm memory was not registered",
        ));
    };
    let payload: T = read_guest_json(
        caller.as_context(),
        &memory,
        ptr,
        len,
        CODE_MODE_WASM_BRIDGE_MAX_BYTES,
    )?;
    let seq = runner_next_seq_blocking()?;
    let output = build(payload, seq)?;
    runner_emit_blocking(output)?;
    caller.data_mut().emitted_operations = caller.data().emitted_operations.saturating_add(1);
    Ok(i32::try_from(seq)?)
}

fn lab_emit_done(mut caller: Caller<'_, WasmRunState>, ptr: i32, len: i32) -> wasmtime::Result<()> {
    let Some(memory) = caller.data().memory else {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm memory was not registered",
        ));
    };
    let payload: DonePayload = read_guest_json(
        caller.as_context(),
        &memory,
        ptr,
        len,
        CODE_MODE_WASM_BRIDGE_MAX_BYTES,
    )?;
    let result = if let Some(message) = payload.error {
        Err(crate::runner::classify_code_mode_rejection_tool_error(
            message,
        ))
    } else if payload.has_result {
        Ok(CodeModeRunnerResult::Json(payload.result))
    } else {
        Ok(CodeModeRunnerResult::Undefined)
    };
    caller.data_mut().done = Some(result);
    Ok(())
}

fn lab_console_log(caller: Caller<'_, WasmRunState>, ptr: i32, len: i32) -> wasmtime::Result<()> {
    let Some(memory) = caller.data().memory else {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm memory was not registered",
        ));
    };
    let message = read_guest_string(
        caller.as_context(),
        &memory,
        ptr,
        len,
        CODE_MODE_WASM_BRIDGE_MAX_BYTES,
    )?;
    eprintln!("{message}");
    Ok(())
}

fn lab_pending_input_len(caller: Caller<'_, WasmRunState>) -> wasmtime::Result<i32> {
    let len = caller
        .data()
        .pending_settlement_input
        .as_deref()
        .map(str::len)
        .unwrap_or(0);
    i32::try_from(len).map_err(Into::into)
}

fn lab_pending_input_copy(
    mut caller: Caller<'_, WasmRunState>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<()> {
    let Some(memory) = caller.data().memory else {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm memory was not registered",
        ));
    };
    if ptr < 0 || len < 0 {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm pending input copy received a negative pointer or length",
        ));
    }
    let input = caller
        .data()
        .pending_settlement_input
        .as_deref()
        .unwrap_or_default()
        .as_bytes()
        .to_vec();
    let len = usize::try_from(len)?;
    if len != input.len() {
        return Err(wasmtime::Error::msg(
            "Code Mode Wasm pending input copy length changed",
        ));
    }
    memory.write(caller.as_context_mut(), ptr as usize, &input)?;
    Ok(())
}

pub(crate) fn settle_pending_operation(
    store: &mut Store<WasmRunState>,
    input: &CodeModeRunnerInput,
) -> Result<(), ToolError> {
    let started = std::time::Instant::now();
    let message = serde_json::to_string(input).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to encode Code Mode runner input for Wasm: {err}"),
    })?;
    store.data_mut().pending_settlement_input = Some(message);
    let result = invoke_plugin_function(store, "__labSettlePendingOperationEntrypoint");
    store.data_mut().pending_settlement_input = None;
    result?;
    store.data_mut().bridge_roundtrip_ms = store
        .data()
        .bridge_roundtrip_ms
        .saturating_add(started.elapsed().as_millis());
    Ok(())
}

pub(crate) fn compile_settlement_entrypoints(
    store: &mut Store<WasmRunState>,
) -> Result<(), ToolError> {
    let source = r"
export function __labSettlePendingOperationEntrypoint() {
  globalThis.__labSettlePendingOperation(globalThis.__labReadPendingInput());
}
export async function __labPumpEventLoop() {
  await Promise.resolve();
}
";
    let (bytecode_ptr, bytecode_len) = compile_plugin_script(store, source)?;
    store.data_mut().settlement_bytecode = Some((bytecode_ptr, bytecode_len));
    Ok(())
}

pub(crate) fn pump_event_loop(store: &mut Store<WasmRunState>) -> Result<(), ToolError> {
    invoke_plugin_function(store, "__labPumpEventLoop")
}

pub(crate) fn invoke_plugin_function(
    store: &mut Store<WasmRunState>,
    function_name: &str,
) -> Result<(), ToolError> {
    let (bytecode_ptr, bytecode_len) =
        store
            .data()
            .settlement_bytecode
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: "Code Mode Wasm settlement bytecode is not registered".to_string(),
            })?;
    let memory = store.data().memory.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "Code Mode Wasm memory is not registered".to_string(),
    })?;
    let cabi_realloc = store
        .data()
        .cabi_realloc
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode Wasm cabi_realloc is not registered".to_string(),
        })?;
    let invoke = store.data().invoke.clone().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "Code Mode Wasm invoke is not registered".to_string(),
    })?;
    let name_bytes = function_name.as_bytes();
    let name_ptr = cabi_realloc
        .call(
            store.as_context_mut(),
            (0, 0, 1, i32::try_from(name_bytes.len()).unwrap()),
        )
        .map_err(wasm_call_error)?;
    memory
        .write(store.as_context_mut(), name_ptr as usize, name_bytes)
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to write Code Mode Wasm function name: {err}"),
        })?;
    invoke
        .call(
            store.as_context_mut(),
            (
                bytecode_ptr,
                bytecode_len,
                1,
                name_ptr,
                i32::try_from(name_bytes.len()).unwrap(),
            ),
        )
        .map_err(wasm_call_error)
}

pub(crate) fn compile_plugin_script(
    store: &mut Store<WasmRunState>,
    script: &str,
) -> Result<(i32, i32), ToolError> {
    let memory = store.data().memory.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "Code Mode Wasm memory is not registered".to_string(),
    })?;
    let cabi_realloc = store
        .data()
        .cabi_realloc
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode Wasm cabi_realloc is not registered".to_string(),
        })?;
    let compile_src = store
        .data()
        .compile_src
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode Wasm compile-src is not registered".to_string(),
        })?;

    let bytes = script.as_bytes();
    if bytes.len() > CODE_MODE_WASM_BRIDGE_MAX_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "Code Mode Wasm settlement script exceeded bridge cap".to_string(),
        });
    }
    let ptr = cabi_realloc
        .call(
            store.as_context_mut(),
            (0, 0, 1, i32::try_from(bytes.len()).unwrap()),
        )
        .map_err(wasm_call_error)?;
    memory
        .write(store.as_context_mut(), ptr as usize, bytes)
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to write Code Mode Wasm settlement script: {err}"),
        })?;
    let ret = compile_src
        .call(
            store.as_context_mut(),
            (ptr, i32::try_from(bytes.len()).unwrap()),
        )
        .map_err(wasm_call_error)?;
    let ret = checked_guest_bytes(store.as_context(), &memory, ret, 12, 12)?;
    let status = i32::from_le_bytes(ret[0..4].try_into().unwrap());
    let bytecode_ptr = i32::from_le_bytes(ret[4..8].try_into().unwrap());
    let bytecode_len = i32::from_le_bytes(ret[8..12].try_into().unwrap());
    if status != 0 {
        let error = read_guest_string(
            store.as_context(),
            &memory,
            bytecode_ptr,
            bytecode_len,
            CODE_MODE_WASM_BRIDGE_MAX_BYTES,
        )?;
        return Err(ToolError::Sdk {
            sdk_kind: "server_error".to_string(),
            message: format!("failed to compile Code Mode Wasm settlement script: {error}"),
        });
    }
    Ok((bytecode_ptr, bytecode_len))
}

fn wasm_call_error(err: wasmtime::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "server_error".to_string(),
        message: format!("Code Mode Wasm bridge call failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Engine, MemoryType};

    fn memory_with(bytes: &[u8]) -> (Store<WasmRunState>, Memory) {
        let engine = Engine::default();
        let mut store = Store::new(&engine, WasmRunState::default());
        let memory = Memory::new(&mut store, MemoryType::new(1, Some(1))).unwrap();
        memory.write(&mut store, 0, bytes).unwrap();
        (store, memory)
    }

    #[test]
    fn rejects_oob_pointer() {
        let (store, memory) = memory_with(b"abc");
        let err = checked_guest_bytes(&store, &memory, 65_536, 1, 10).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("outside guest memory"));
    }

    #[test]
    fn rejects_before_copying_when_payload_exceeds_cap() {
        let (store, memory) = memory_with(b"abc");
        let err = checked_guest_bytes(&store, &memory, 0, 65_536, 2).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("exceeded 2 bytes"));
    }

    #[test]
    fn rejects_pointer_overflow() {
        let (store, memory) = memory_with(b"abc");
        let err = checked_guest_bytes(
            &store,
            &memory,
            i32::MAX,
            16,
            CODE_MODE_WASM_BRIDGE_MAX_BYTES,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn rejects_non_utf8_string() {
        let (store, memory) = memory_with(&[0xff, 0xff]);
        let err = read_guest_string(&store, &memory, 0, 2, 10).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("not UTF-8"));
    }

    #[test]
    fn rejects_malformed_json() {
        let (store, memory) = memory_with(b"{nope");
        let err = read_guest_json::<_, Value>(&store, &memory, 0, 5, 10).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("valid JSON"));
    }
}
