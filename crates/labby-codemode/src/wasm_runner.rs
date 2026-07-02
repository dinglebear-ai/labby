//! Wasmtime-backed execution for one Code Mode runner subprocess.

use std::num::NonZeroUsize;
use std::time::Duration;

use anyhow::{Context, Result};
use lru::LruCache;
use sha2::{Digest, Sha256};
use wasmtime::{Extern, Instance, Linker, Module, ResourceLimiter, Store};
use wasmtime_wasi::p1;

use crate::config::MAX_SOURCE_BYTES;
use crate::error::ToolError;
use crate::protocol::CodeModeRunnerResult;
use crate::runner_io::runner_read_input_blocking;
use crate::wasm_bridge::{WasmRunState, install_lab_imports, settle_pending_operation};
use crate::wasm_codegen::{compile_code_mode_wasm, wrap_code_mode_for_wasm};
use crate::wasm_plugin::{
    CODE_MODE_WASM_DEFAULT_FUEL, CODE_MODE_WASM_EPOCH_TICK_MS, CODE_MODE_WASM_MEMORY_LIMIT_BYTES,
    CODE_MODE_WASM_PLUGIN_HASH, WasmPlugin, load_wasm_plugin, wasm_limits_disabled,
};

const MODULE_CACHE_CAPACITY: usize = 32;
const DEFAULT_RUNNER_TIMEOUT: Duration = Duration::from_secs(30);
const CODE_MODE_WASM_TABLE_LIMIT_ELEMENTS: usize = 16 * 1024;

pub(crate) struct WasmExecutionTimings {
    pub(crate) wrap_ms: u128,
    pub(crate) javy_codegen_ms: u128,
    pub(crate) wasm_module_compile_ms: u128,
    pub(crate) plugin_instantiate_ms: u128,
    pub(crate) generated_instantiate_ms: u128,
    pub(crate) bridge_roundtrip_ms: u128,
}

pub(crate) struct WasmRunner {
    plugin: WasmPlugin,
    module_cache: LruCache<[u8; 32], Module>,
    _epoch_ticker: std::thread::JoinHandle<()>,
}

impl WasmRunner {
    pub(crate) fn new() -> Result<Self> {
        let plugin = load_wasm_plugin()?;
        let _epoch_ticker = spawn_epoch_ticker(plugin.engine.clone());
        Ok(Self {
            plugin,
            module_cache: LruCache::new(NonZeroUsize::new(MODULE_CACHE_CAPACITY).unwrap()),
            _epoch_ticker,
        })
    }

    pub(crate) fn execute(
        &mut self,
        code: &str,
        proxy: &str,
        timeout: Duration,
    ) -> Result<CodeModeRunnerResult, ToolError> {
        let timeout = if timeout.is_zero() {
            DEFAULT_RUNNER_TIMEOUT
        } else {
            timeout
        };
        if code.len().saturating_add(proxy.len()) > MAX_SOURCE_BYTES {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: format!("Code Mode source exceeded {MAX_SOURCE_BYTES} bytes"),
            });
        }

        let wrap_started = std::time::Instant::now();
        let wrapped = wrap_code_mode_for_wasm(code, proxy);
        let wrap_ms = wrap_started.elapsed().as_millis();
        let cache_key = module_cache_key(&wrapped);

        let codegen_started = std::time::Instant::now();
        let mut wasm_module_compile_ms = 0;
        let module = if let Some(module) = self.module_cache.get(&cache_key) {
            module.clone()
        } else {
            let wasm = generate_wasm_on_blocking_thread(
                self.plugin.plugin.clone(),
                code.to_string(),
                proxy.to_string(),
                timeout.saturating_sub(Duration::from_millis(50)),
            )?;
            let compile_started = std::time::Instant::now();
            let module =
                Module::from_binary(&self.plugin.engine, &wasm).map_err(|err| ToolError::Sdk {
                    sdk_kind: "server_error".to_string(),
                    message: format!("failed to compile generated Code Mode Wasm module: {err}"),
                })?;
            validate_generated_imports(&module, &self.plugin.import_namespace)?;
            wasm_module_compile_ms = compile_started.elapsed().as_millis();
            self.module_cache.put(cache_key, module.clone());
            eprintln!("codemode_wasmtime wasm_module_compile_ms={wasm_module_compile_ms}");
            module
        };
        let javy_codegen_ms = codegen_started.elapsed().as_millis();

        let mut store = Store::new(&self.plugin.engine, WasmRunState::default());
        if !wasm_limits_disabled() {
            store
                .set_fuel(CODE_MODE_WASM_DEFAULT_FUEL)
                .map_err(wasm_error)?;
            store.set_epoch_deadline(epoch_deadline_ticks(timeout));
        }
        store.limiter(|state| &mut state.limiter);

        let mut linker = Linker::new(&self.plugin.engine);
        p1::add_to_linker_sync(&mut linker, |state: &mut WasmRunState| &mut state.wasi)
            .map_err(wasm_error)?;
        install_lab_imports(&mut linker, &self.plugin.import_namespace).map_err(wasm_error)?;

        let plugin_started = std::time::Instant::now();
        let plugin_instance = linker
            .instantiate(&mut store, &self.plugin.module)
            .map_err(wasm_error)?;
        let plugin_instantiate_ms = plugin_started.elapsed().as_millis();

        register_plugin_exports(
            &mut store,
            &mut linker,
            &self.plugin.import_namespace,
            &plugin_instance,
        )?;

        let generated_started = std::time::Instant::now();
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(wasm_error)?;
        let generated_instantiate_ms = generated_started.elapsed().as_millis();
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .map_err(wasm_error)?;
        start.call(&mut store, ()).map_err(classify_wasm_trap)?;

        while store.data().done.is_none() {
            let input = runner_read_input_blocking()?;
            settle_pending_operation(&mut store, &input)?;
        }

        let fuel_remaining = if wasm_limits_disabled() {
            0
        } else {
            store.get_fuel().unwrap_or(0)
        };
        let fuel_consumed = CODE_MODE_WASM_DEFAULT_FUEL.saturating_sub(fuel_remaining);
        let timings = WasmExecutionTimings {
            wrap_ms,
            javy_codegen_ms,
            wasm_module_compile_ms,
            plugin_instantiate_ms,
            generated_instantiate_ms,
            bridge_roundtrip_ms: store.data().bridge_roundtrip_ms,
        };
        eprintln!(
            "codemode_wasmtime runtime=wasmtime wrap_ms={} javy_codegen_ms={} wasm_module_compile_ms={} plugin_instantiate_ms={} generated_instantiate_ms={} bridge_roundtrip_ms={} fuel_remaining={} fuel_consumed={}",
            timings.wrap_ms,
            timings.javy_codegen_ms,
            timings.wasm_module_compile_ms,
            timings.plugin_instantiate_ms,
            timings.generated_instantiate_ms,
            timings.bridge_roundtrip_ms,
            fuel_remaining,
            fuel_consumed
        );

        store
            .data_mut()
            .done
            .take()
            .context("Code Mode Wasm finished without a result")
            .map_err(wasm_error)?
    }
}

fn generate_wasm_on_blocking_thread(
    plugin: javy_codegen::Plugin,
    code: String,
    proxy: String,
    timeout: Duration,
) -> Result<Vec<u8>, ToolError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "server_error".to_string(),
                message: format!("failed to create Code Mode Wasm codegen runtime: {err}"),
            })
            .and_then(|rt| {
                rt.block_on(compile_code_mode_wasm(&plugin, &code, &proxy))
                    .map_err(|err| ToolError::Sdk {
                        sdk_kind: "invalid_param".to_string(),
                        message: format!("failed to generate Code Mode Wasm module: {err}"),
                    })
            });
        drop(tx.send(result));
    });
    rx.recv_timeout(timeout).map_err(|err| {
        let (sdk_kind, message) = match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => (
                "timeout",
                "Code Mode Wasm code generation timed out".to_string(),
            ),
            std::sync::mpsc::RecvTimeoutError::Disconnected => (
                "server_error",
                "Code Mode Wasm code generation worker exited".to_string(),
            ),
        };
        ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message,
        }
    })?
}

fn register_plugin_exports(
    store: &mut Store<WasmRunState>,
    linker: &mut Linker<WasmRunState>,
    namespace: &str,
    plugin_instance: &Instance,
) -> Result<(), ToolError> {
    let memory = plugin_instance
        .get_memory(&mut *store, "memory")
        .context("Javy plugin did not export memory")
        .map_err(wasm_error)?;
    let cabi_realloc = plugin_instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, "cabi_realloc")
        .map_err(|err| wasm_error(format!("Javy plugin did not export cabi_realloc: {err}")))?;
    let compile_src = plugin_instance
        .get_typed_func::<(i32, i32), i32>(&mut *store, "compile-src")
        .map_err(|err| wasm_error(format!("Javy plugin did not export compile-src: {err}")))?;
    let invoke = plugin_instance
        .get_typed_func::<(i32, i32, i32, i32, i32), ()>(&mut *store, "invoke")
        .map_err(|err| wasm_error(format!("Javy plugin did not export invoke: {err}")))?;

    linker
        .define(&mut *store, namespace, "memory", Extern::Memory(memory))
        .map_err(wasm_error)?;
    linker
        .define(
            &mut *store,
            namespace,
            "cabi_realloc",
            Extern::Func(*cabi_realloc.func()),
        )
        .map_err(wasm_error)?;
    linker
        .define(
            &mut *store,
            namespace,
            "invoke",
            Extern::Func(*invoke.func()),
        )
        .map_err(wasm_error)?;

    let data = store.data_mut();
    data.memory = Some(memory);
    data.cabi_realloc = Some(cabi_realloc);
    data.compile_src = Some(compile_src);
    data.invoke = Some(invoke);
    Ok(())
}

fn validate_generated_imports(module: &Module, namespace: &str) -> Result<(), ToolError> {
    for import in module.imports() {
        let allowed = import.module() == namespace
            && matches!(import.name(), "memory" | "cabi_realloc" | "invoke");
        if !allowed {
            return Err(ToolError::Sdk {
                sdk_kind: "server_error".to_string(),
                message: format!(
                    "unexpected Wasm import {}::{} in generated Code Mode module",
                    import.module(),
                    import.name()
                ),
            });
        }
    }
    Ok(())
}

fn module_cache_key(wrapped_source: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(wrapped_source.as_bytes());
    hasher.update(CODE_MODE_WASM_PLUGIN_HASH.as_bytes());
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(b"dynamic-javy-v1");
    hasher.finalize().into()
}

fn epoch_deadline_ticks(timeout: Duration) -> u64 {
    let ticks = timeout.as_millis() / u128::from(CODE_MODE_WASM_EPOCH_TICK_MS);
    u64::try_from(ticks.saturating_sub(1))
        .unwrap_or(u64::MAX)
        .max(1)
}

fn spawn_epoch_ticker(engine: wasmtime::Engine) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(CODE_MODE_WASM_EPOCH_TICK_MS));
            engine.increment_epoch();
        }
    })
}

#[derive(Default)]
pub(crate) struct CodeModeLimiter;

impl ResourceLimiter for CodeModeLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(desired <= CODE_MODE_WASM_MEMORY_LIMIT_BYTES)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        Ok(desired <= CODE_MODE_WASM_TABLE_LIMIT_ELEMENTS)
    }
}

fn classify_wasm_trap(err: wasmtime::Error) -> ToolError {
    let message = err.to_string();
    let is_timeout = message.contains("all fuel consumed") || message.contains("epoch deadline");
    ToolError::Sdk {
        sdk_kind: if is_timeout {
            "timeout".to_string()
        } else {
            "server_error".to_string()
        },
        message: if is_timeout {
            format!("Code Mode execution timed out: {message}")
        } else {
            format!("Code Mode Wasm execution failed: {message}")
        },
    }
}

fn wasm_error(err: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "server_error".to_string(),
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_deadline_tracks_timeout() {
        assert_eq!(epoch_deadline_ticks(Duration::from_millis(50)), 1);
        assert_eq!(epoch_deadline_ticks(Duration::from_secs(5)), 49);
    }

    #[test]
    fn generated_module_rejects_unexpected_imports() {
        let engine = wasmtime::Engine::default();
        let wasm = wat::parse_str(r#"(module (import "wasi:filesystem" "open" (func)))"#).unwrap();
        let module = Module::from_binary(&engine, &wasm).unwrap();
        let err = validate_generated_imports(&module, "labby-codemode-plugin-v1").unwrap_err();
        assert_eq!(err.kind(), "server_error");
        assert!(err.to_string().contains("unexpected Wasm import"));
    }
}
