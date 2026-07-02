//! Javy plugin bytes and shared Wasmtime engine/module setup for Code Mode.
//!
//! The plugin is shared per runner subprocess. Each execution still receives a
//! fresh Store and generated JS module, so JS state isolation remains per Start.

use std::borrow::Cow;

use anyhow::{Context, Result};
use javy_codegen::Plugin;
use wasmtime::{Config, Engine, Module};

pub(crate) const CODE_MODE_WASM_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const CODE_MODE_WASM_EPOCH_TICK_MS: u64 = 100;
pub(crate) const CODE_MODE_WASM_DEFAULT_FUEL: u64 = 10_000_000;
pub(crate) const CODE_MODE_WASM_PLUGIN_HASH: &str = env!("LABBY_CODEMODE_PLUGIN_SHA256");

pub(crate) struct WasmPlugin {
    pub(crate) engine: Engine,
    pub(crate) plugin: Plugin,
    pub(crate) module: Module,
    pub(crate) import_namespace: String,
}

pub(crate) fn wasm_limits_disabled() -> bool {
    wasm_limits_disabled_from(std::env::var("LAB_CODE_MODE_WASM_LIMITS").ok().as_deref())
}

pub(crate) fn wasm_limits_disabled_from(value: Option<&str>) -> bool {
    value
        .map(|value| value == "0" || value.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

pub(crate) fn build_wasm_engine() -> Result<Engine> {
    let mut config = Config::new();
    let limits_enabled = !wasm_limits_disabled();
    config.consume_fuel(limits_enabled);
    config.epoch_interruption(limits_enabled);
    config.max_wasm_stack(256 * 1024);
    Engine::new(&config)
        .map_err(|err| anyhow::anyhow!("failed to create Code Mode Wasmtime engine: {err}"))
}

pub(crate) fn load_wasm_plugin_from_bytes(bytes: &'static [u8]) -> Result<WasmPlugin> {
    let plugin = Plugin::new(Cow::Borrowed(bytes)).context("failed to validate Javy plugin")?;
    let import_namespace = plugin_import_namespace(&plugin)?;
    let engine = build_wasm_engine()?;
    let module = Module::from_binary(&engine, plugin.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to compile Javy plugin: {err}"))?;
    Ok(WasmPlugin {
        engine,
        plugin,
        module,
        import_namespace,
    })
}

fn plugin_import_namespace(plugin: &Plugin) -> Result<String> {
    let module = walrus::Module::from_buffer(plugin.as_bytes())
        .context("failed to inspect Javy plugin import namespace")?;
    let section = module
        .customs
        .iter()
        .find_map(|(_, section)| (section.name() == "import_namespace").then_some(section))
        .context("Javy plugin is missing import_namespace custom section")?;
    let bytes = section.data(&Default::default());
    std::str::from_utf8(&bytes)
        .map(str::to_owned)
        .context("Javy plugin import_namespace is not UTF-8")
}

pub(crate) fn load_wasm_plugin() -> Result<WasmPlugin> {
    load_wasm_plugin_from_bytes(include_bytes!(concat!(env!("OUT_DIR"), "/plugin.wasm")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_switch_defaults_to_enabled() {
        assert!(!wasm_limits_disabled_from(None));
        assert!(wasm_limits_disabled_from(Some("0")));
        assert!(wasm_limits_disabled_from(Some("false")));
        assert!(!wasm_limits_disabled_from(Some("1")));
    }

    #[test]
    fn plugin_loads_with_expected_namespace() {
        let plugin = load_wasm_plugin().unwrap();
        assert_eq!(plugin.import_namespace, "labby-codemode-plugin-v1");
        assert!(!plugin.plugin.as_bytes().is_empty());
        assert_eq!(CODE_MODE_WASM_PLUGIN_HASH.len(), 64);
    }
}
