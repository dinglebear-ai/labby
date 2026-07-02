# Code Mode Wasmtime Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Code Mode runner's native QuickJS hot path with Javy-generated Wasm executed by Wasmtime inside the existing runner subprocess, preserving the caller-facing JS API and parent-owned stdio authority boundary.

**Architecture:** Keep the current OS subprocess pool as the outer containment ring. Inside each runner subprocess, build one shared Wasmtime `Engine` and one preinitialized Lab-owned Javy plugin `Module`, then create a fresh `Store`/generated JS `Module`/`Instance` per `Start` message. Host authority stays in the parent process: the Lab-owned Javy plugin registers the same bridge globals the native runner used, and those plugin functions call Wasmtime imports that only emit existing `CodeModeRunnerOutput::{ToolCall, ArtifactWrite, SnippetResolve}` messages and consume matching existing `CodeModeRunnerInput` replies.

**Tech Stack:** Rust 2024, Tokio, Javy v9 `javy-codegen = 4.0.1-alpha.1` from the `v9.0.0` Git tag, `wasmtime = 45.0.3`, `wasmtime-wasi = 45.0.3`, `wasmtime-wizer = 45.0.3`, existing Labby runner stdio protocol.

## Global Constraints

- Scope is issue 168 / bead `lab-crav6`: Code Mode Wasmtime dual sandbox.
- Accepted framing is "defense-in-depth + graceful interruption"; do not claim the current subprocess runner lacks a clean kill mechanism.
- Wasmtime runs inside the existing child runner subprocess; parent-side broker, `CodeModeHost`, OAuth/upstream pools, snippets, and artifact persistence remain parent-owned.
- Per Code Mode execution, exactly one JS engine runs caller code: QuickJS compiled to Wasm via Javy and executed under Wasmtime. Do not run native `javy::Runtime` redundantly.
- No native QuickJS fallback mode in v1. `LAB_CODE_MODE_WASM_LIMITS=0`, if added, disables fuel/epoch limits only while keeping the Wasmtime path.
- Preserve the existing parent/runner line-delimited JSON protocol in `crates/labby-codemode/src/protocol.rs`.
- Preserve the existing caller-facing JavaScript contract: `callTool`, `writeArtifact`, `codemode.search`, `codemode.describe`, `codemode.run`, `codemode.step`, binary result encoding, snippet budgets, and structured `{kind,message}` rejections.
- Caller-facing timeout kind remains `timeout` for fuel exhaustion, epoch interruption, and parent wall-clock timeout; internal logs may include `trap_cause`.
- Keep `labby-codemode` under `#![forbid(unsafe_code)]`.
- Do not add business logic to CLI, MCP, or HTTP adapters.
- Do not grow already-large runner files casually. New Wasmtime code belongs in focused sibling files under `crates/labby-codemode/src/`.
- Security gates are blocking: `cargo deny check` must pass; `cargo audit` must not report Wasmtime/WASI 42 advisories introduced by this work.
- Baseline `cargo audit` advisories for unrelated workspace dependencies may remain if unchanged, but must be called out as baseline.
- Current dependency proof branch pins Javy v9 codegen and Wasmtime/WASI 45.0.3 because crates.io `javy-codegen = 4.0.0` pulls vulnerable Wasmtime/WASI 42.

---

## File Structure

- Modify `crates/labby-codemode/Cargo.toml`: keep the proven Javy v9/Wasmtime 45.0.3 dependency set; add build dependencies only if the chosen plugin build path requires them.
- Modify `deny.toml`: preserve the explicit Javy Git source allow-list and the existing scoped `anyhow` wrapper policy for the Javy/Wasmtime codegen boundary.
- Create `crates/labby-codemode/javy-plugin/`: Lab-owned Javy plugin source that registers bridge globals (`__labEmitToolCall`, `__labEmitArtifactWrite`, `__labEmitSnippetResolve`, `__labEmitDone`) and delegates them to Wasmtime imports; no parent-side `CodeModeHost` authority moves into this plugin.
- Create `crates/labby-codemode/src/wasm_plugin.rs`: load and validate the preinitialized Lab Javy plugin bytes, expose the Javy import namespace, create the shared Wasmtime engine/module, and provide dependency/cache-key metadata.
- Create `crates/labby-codemode/src/wasm_codegen.rs`: compile a wrapped Code Mode JS string into a Javy dynamic-linking Wasm module.
- Create `crates/labby-codemode/src/wasm_bridge.rs`: bounded guest linear-memory reads/writes, Javy IO bridge handling, pending-operation settlement, final-result capture, and protocol emission.
- Create `crates/labby-codemode/src/wasm_runner.rs`: per-`Start` Wasmtime store/module/instance execution, fuel/epoch/resource limits, trap classification, and reusable execution errors.
- Modify `crates/labby-codemode/src/runner.rs`: keep the subprocess loop, `PR_SET_DUMPABLE`, sequence reset, cwd jail reset, wrapping helpers, and error classification; replace native `javy::Runtime` construction with `wasm_runner`.
- Modify `crates/labby-codemode/src/lib.rs`: register new modules and update crate-level runtime docs.
- Modify `crates/labby-codemode/src/runner_io.rs`: add a small runner-side blocking protocol helper used by the Wasm bridge, so `wasm_bridge.rs` does not call back into `runner.rs`.
- Modify `crates/labby-codemode/src/protocol.rs`: only for internal helper types if needed; do not change wire shapes.
- Modify `crates/labby-codemode/CLAUDE.md`: update runtime, invariants, file responsibilities, and rules.
- Modify `docs/dev/CODE_MODE.md`, `docs/dev/ERRORS.md`, `docs/dev/OBSERVABILITY.md`, and `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`: make docs match the implemented runtime and verified dependency path.

## Engineering Review Amendments Applied

These amendments are part of the plan and supersede any lower-level sketch that conflicts with them.

RECOMMENDATIONS APPLIED:

[x] 1. Preserve pending-promise fan-out semantics. The Wasm path must not replace `callTool`/`writeArtifact`/`codemode.run` with a blocking request/response helper. JS bridge calls must emit a seq immediately, store a pending promise, and settle later from existing `CodeModeRunnerInput` replies.

[x] 2. Use one explicit guest-host ABI. The v1 ABI is a Lab-owned Javy plugin, not default Javy stream IO. The plugin registers bridge globals in QuickJS and backs them with Wasmtime imports implemented in `wasm_bridge.rs`. Those imports emit existing runner protocol messages to the parent and return seq IDs immediately, preserving the current pending-promise model.

[x] 3. Add an explicit final-result channel. The wrapper must send `kind = "done"` through the same bridge with `{ state: "json", value }` or `{ state: "undefined" }`; `WasmRunState` stores that result and `wasm_runner` emits the existing `Done` envelope from it. Delete the speculative `__lab_result_ptr` / `__lab_result_len` global-pointer mechanism.

[x] 4. Narrow trap classification. Only Wasmtime fuel exhaustion and epoch interruption map to caller-facing `timeout`. Unexpected Wasm traps, missing imports/exports, bridge ABI failures, memory faults, allocation failures, plugin load failures, and module validation failures map to `server_error` or `invalid_param` and evict the runner unless explicitly proven reusable.

[x] 5. Add generated-module import allow-listing before instantiation. Reject generated modules that import anything except the expected Javy dynamic-link namespace and the explicitly approved stream-IO/WASI pieces required by the validated plugin path. Add a regression test proving no `wasi:*` filesystem/env/socket imports are introduced accidentally.

[x] 6. Bound pre-execution work. Enforce source/proxy size limits before Javy codegen, add a compile/codegen timeout under the runner's parent deadline, and map oversized source or compile timeout to structured errors. Fuel/epoch does not protect codegen or Wasmtime module compilation.

[x] 7. Validate all guest-memory reads and response writes. The bridge must reject negative pointers, pointer overflow, OOB reads, OOB output slots, huge lengths, non-UTF-8, and malformed JSON without panic. Cap-before-copy tests must use attacker-sized lengths, not tiny toy values only.

[x] 8. Add plugin provenance and ABI attestation. Vendored or generated `plugin.wasm` bytes must have a checked SHA256/provenance record, build scripts must assert the hash, and tests must validate expected exports plus import namespace. If building from source, use `cargo build --locked --offline` where possible and run the actual `wasmtime-wizer` preinitialization step.

[x] 9. Add latency instrumentation and a benchmark gate. Record `wrap_ms`, `javy_codegen_ms`, `wasm_module_compile_ms`, `plugin_instantiate_ms`, `generated_instantiate_ms`, `bridge_roundtrip_ms`, total warm p50/p95, `fuel_consumed`, and payload sizes. PR handoff must include these numbers and an explicit pass/fail statement.

[x] 10. Add a bounded module cache or a measured non-goal. V1 should cache compiled `wasmtime::Module` values per runner using an LRU keyed by `sha256(wrapped_source + proxy + plugin_hash + javy_version + codegen_options)`, while still creating a fresh `Store`/`Instance` per execution. If cache implementation proves too much for v1, benchmark data must justify deferral before implementation proceeds.

[x] 11. Tie epoch and bridge deadlines to the caller timeout. Epoch deadline ticks and bridge wait deadlines derive from `RunnerConfig.timeout` with headroom below the parent kill deadline, not a hardcoded 30 seconds used in every test.

[x] 12. Make workspace verification mandatory. `cargo build --workspace --all-features` is a blocking gate, not "if time permits".

## Task 1: Lock Dependencies And Plugin Artifact Strategy

**Files:**
- Modify: `crates/labby-codemode/Cargo.toml`
- Modify: `deny.toml`
- Modify: `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`
- Create: `crates/labby-codemode/javy-plugin/Cargo.toml`
- Create: `crates/labby-codemode/javy-plugin/src/lib.rs`
- Create: `crates/labby-codemode/build-support/Cargo.toml`
- Create: `crates/labby-codemode/build-support/src/lib.rs`
- Create: `crates/labby-codemode/build.rs`
- Create: `crates/labby-codemode/plugin.sha256`
- Create: `crates/labby-codemode/src/wasm_plugin.rs`
- Test: `crates/labby-codemode/src/wasm_plugin.rs`

**Interfaces:**
- Consumes: Javy v9 source facts: `javy-codegen::Plugin::new(Cow<'static, [u8]>)`, `Plugin::as_bytes()`, `Generator::new(plugin)`, `Generator::linking(LinkingKind::Dynamic)`, `Generator::generate(&JS).await`.
- Produces:
  - `pub(crate) struct WasmPlugin { pub(crate) engine: wasmtime::Engine, pub(crate) plugin: javy_codegen::Plugin, pub(crate) module: wasmtime::Module, pub(crate) import_namespace: String }`
  - `pub(crate) fn wasm_limits_disabled() -> bool`
  - `pub(crate) fn build_wasm_engine() -> anyhow::Result<wasmtime::Engine>`
  - `pub(crate) fn load_wasm_plugin() -> anyhow::Result<WasmPlugin>`

- [ ] **Step 1: Confirm the dependency graph still proves the unblocked path**

Run:

```bash
cargo tree -p labby-codemode -i javy-codegen
cargo tree -p labby-codemode -i wasmtime@45.0.3
cargo tree -p labby-codemode -i wasmtime-wasi@45.0.3
cargo tree -p labby-codemode -i wasmtime@42.0.2 || true
```

Expected:

```text
javy-codegen v4.0.1-alpha.1 (https://github.com/bytecodealliance/javy?tag=v9.0.0#...)
...
wasmtime v45.0.3
...
wasmtime-wasi v45.0.3
...
error: package ID specification `wasmtime@42.0.2` did not match any packages
```

- [ ] **Step 2: Add the initial plugin loader module**

Edit `crates/labby-codemode/src/wasm_plugin.rs`:

```rust
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
pub(crate) const CODE_MODE_WASM_EPOCH_DEADLINE_TICKS: u64 = 300;
pub(crate) const CODE_MODE_WASM_DEFAULT_FUEL: u64 = 10_000_000;

pub(crate) struct WasmPlugin {
    pub(crate) engine: Engine,
    pub(crate) plugin: Plugin,
    pub(crate) module: Module,
    pub(crate) import_namespace: String,
}

pub(crate) fn wasm_limits_disabled() -> bool {
    std::env::var("LAB_CODE_MODE_WASM_LIMITS")
        .map(|value| value == "0" || value.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

pub(crate) fn build_wasm_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.consume_fuel(!wasm_limits_disabled());
    config.epoch_interruption(!wasm_limits_disabled());
    config.max_wasm_stack(256 * 1024);
    Engine::new(&config).context("failed to create Code Mode Wasmtime engine")
}

pub(crate) fn load_wasm_plugin_from_bytes(bytes: &'static [u8]) -> Result<WasmPlugin> {
    let plugin = Plugin::new(Cow::Borrowed(bytes)).context("failed to validate Javy plugin")?;
    let import_namespace = plugin_import_namespace(&plugin)?;
    let engine = build_wasm_engine()?;
    let module =
        Module::from_binary(&engine, plugin.as_bytes()).context("failed to compile Javy plugin")?;
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
        unsafe {
            std::env::remove_var("LAB_CODE_MODE_WASM_LIMITS");
        }
        assert!(!wasm_limits_disabled());
    }
}
```

If the workspace forbids `unsafe` env mutation in tests under current Rust, replace the env test with a pure helper:

```rust
fn wasm_limits_disabled_from(value: Option<&str>) -> bool {
    value
        .map(|value| value == "0" || value.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}
```

and call that helper from `wasm_limits_disabled`.

- [ ] **Step 3: Add required direct dependencies for the loader**

Modify `crates/labby-codemode/Cargo.toml` so `[dependencies]` includes:

```toml
walrus = "0.25.2"
```

If `walrus` is already pulled transitively, keep it direct because `wasm_plugin.rs` inspects the plugin custom section directly.

- [ ] **Step 4: Decide and implement the plugin artifact source**

Preferred v1 path: build the Lab-owned plugin from source at compile time, run the real `wasmtime-wizer` preinitialization step, write the preinitialized bytes to `OUT_DIR/plugin.wasm`, and assert the SHA256 recorded in `crates/labby-codemode/plugin.sha256` when that file is non-empty. Do not use the default Javy plugin because it does not register Lab's bridge globals and would force a stream-IO protocol rewrite.

Create `crates/labby-codemode/javy-plugin/Cargo.toml`:

```toml
[package]
name = "labby-codemode-javy-plugin"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
name = "labby_codemode_javy_plugin"
crate-type = ["cdylib"]

[dependencies]
anyhow = "1"
javy-plugin-api = { git = "https://github.com/bytecodealliance/javy", tag = "v9.0.0", features = ["json"] }
serde = { workspace = true }
serde_json = { workspace = true }
```

Create `crates/labby-codemode/javy-plugin/src/lib.rs`:

```rust
use javy_plugin_api::javy::{Ctx, Function, Runtime};
use javy_plugin_api::{Config, import_namespace};

import_namespace!("labby-codemode-plugin-v1");

#[link(wasm_import_module = "labby-codemode-plugin-v1")]
unsafe extern "C" {
    fn lab_emit_tool_call(ptr: i32, len: i32) -> i32;
    fn lab_emit_artifact_write(ptr: i32, len: i32) -> i32;
    fn lab_emit_snippet_resolve(ptr: i32, len: i32) -> i32;
    fn lab_emit_done(ptr: i32, len: i32);
}

fn config() -> Config {
    let mut config = Config::default();
    config
        .text_encoding(true)
        .javy_stream_io(false)
        .simd_json_builtins(true);
    config
}

fn modify_runtime(runtime: Runtime) -> Runtime {
    runtime.context().with(|cx| {
        let globals = cx.globals();
        globals
            .set("__labEmitToolCall", Function::new(cx.clone(), |payload: String| emit_tool(payload)))
            .unwrap();
        globals
            .set(
                "__labEmitArtifactWrite",
                Function::new(cx.clone(), |payload: String| emit_artifact(payload)),
            )
            .unwrap();
        globals
            .set(
                "__labEmitSnippetResolve",
                Function::new(cx.clone(), |payload: String| emit_snippet(payload)),
            )
            .unwrap();
        globals
            .set("__labEmitDone", Function::new(cx.clone(), |payload: String| emit_done(payload)))
            .unwrap();
    });
    runtime
}

fn emit_tool(payload: String) -> i32 {
    unsafe { lab_emit_tool_call(payload.as_ptr() as i32, payload.len() as i32) }
}

fn emit_artifact(payload: String) -> i32 {
    unsafe { lab_emit_artifact_write(payload.as_ptr() as i32, payload.len() as i32) }
}

fn emit_snippet(payload: String) -> i32 {
    unsafe { lab_emit_snippet_resolve(payload.as_ptr() as i32, payload.len() as i32) }
}

fn emit_done(payload: String) {
    unsafe { lab_emit_done(payload.as_ptr() as i32, payload.len() as i32) }
}

#[unsafe(export_name = "initialize-runtime")]
fn initialize_runtime() {
    javy_plugin_api::initialize_runtime(config, modify_runtime).unwrap();
}
```

If `javy-plugin-api` requires different `Function::new` signatures at v9, update this code from the checked-out Javy v9 plugin examples and keep the same exported JS global names and import names.

Create `crates/labby-codemode/build.rs`:

```rust
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=javy-plugin/Cargo.toml");
    println!("cargo:rerun-if-changed=javy-plugin/src/lib.rs");
    println!("cargo:rerun-if-changed=plugin.sha256");
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            "javy-plugin/Cargo.toml",
            "--target",
            "wasm32-wasip1",
            "--release",
            "--locked",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "failed to build Javy plugin");
    let raw = std::fs::read("javy-plugin/target/wasm32-wasip1/release/labby_codemode_javy_plugin.wasm").unwrap();
    let initialized = labby_build_support::preinitialize_javy_plugin(&raw).unwrap();
    let actual = labby_build_support::sha256_hex(&initialized);
    let expected = std::fs::read_to_string("plugin.sha256").unwrap_or_default();
    let expected = expected.trim();
    if !expected.is_empty() && expected != actual {
        panic!("preinitialized plugin hash mismatch: expected {expected}, got {actual}");
    }
    std::fs::write(out.join("plugin.wasm"), initialized).unwrap();
    println!("cargo:warning=labby Code Mode plugin sha256 {actual}");
}
```

Create `crates/labby-codemode/build-support/Cargo.toml`:

```toml
[package]
name = "labby-codemode-build-support"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
anyhow = "1"
sha2 = "0.11"
hex = "0.4"
wasmtime = "=45.0.3"
wasmtime-wasi = "=45.0.3"
wasmtime-wizer = { version = "=45.0.3", features = ["wasmtime"] }
deterministic-wasi-ctx = "=4.0.3"
tokio = { workspace = true, features = ["rt"] }
```

Create `crates/labby-codemode/build-support/src/lib.rs`:

```rust
use anyhow::Result;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Store};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wizer::Wizer;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn preinitialize_javy_plugin(bytes: &[u8]) -> Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let engine = Engine::default();
        let mut builder = WasiCtxBuilder::new();
        deterministic_wasi_ctx::add_determinism_to_wasi_ctx_builder(&mut builder);
        let wasi = builder.build_p1();
        let mut store = Store::new(&engine, wasi);
        Wizer::new()
            .init_func("initialize-runtime")
            .keep_init_func(true)
            .run(&mut store, bytes, async |store, module| {
                let engine = store.engine();
                let mut linker = Linker::new(engine);
                wasmtime_wasi::p1::add_to_linker_async(&mut linker, |cx| cx)?;
                linker.define_unknown_imports_as_traps(module)?;
                linker.instantiate_async(store, module).await
            })
            .await
            .map_err(Into::into)
    })
}
```

If the build helper cannot link `wasmtime-wizer` cleanly from `build.rs`, move the helper into a tiny build-support crate and call it from `build.rs`; do not silently fall back to copying an unverified binary. If CI lacks `wasm32-wasip1`, add an explicit setup step or document the target requirement in the PR; do not make the build fetch network artifacts.

- [ ] **Step 5: Run the plugin-loader check**

Run:

```bash
cargo test -p labby-codemode wasm_plugin --all-features -- --nocapture
```

Expected:

```text
test result: ok.
```

- [ ] **Step 6: Run dependency gates immediately**

Run:

```bash
cargo deny check
cargo audit
```

Expected:

```text
cargo deny check
... success ...
```

`cargo audit` may still report the existing workspace baseline advisories for `quinn-proto`, `rsa`, and the allowed `paste` warning. It must not report new Wasmtime/WASI 42 advisories.

- [ ] **Step 7: Commit**

```bash
git add crates/labby-codemode/Cargo.toml crates/labby-codemode/src/wasm_plugin.rs deny.toml docs/dev/CODE_MODE_WASMTIME_SPIKE.md
git commit -m "build(codemode): lock javy wasmtime plugin path"
```

## Task 2: Compile Wrapped Code Mode JS To Dynamic Javy Wasm

**Files:**
- Create: `crates/labby-codemode/src/wasm_codegen.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Test: `crates/labby-codemode/src/wasm_codegen.rs`

**Interfaces:**
- Consumes: `wasm_plugin::WasmPlugin`, `wrapper::CODE_MODE_VALUE_CODEC_JS`, `wrapper::code_mode_main_invoker`.
- Produces:
  - `pub(crate) async fn compile_code_mode_wasm(plugin: &javy_codegen::Plugin, code: &str, proxy: &str) -> anyhow::Result<Vec<u8>>`
  - `pub(crate) fn wrap_code_mode_for_wasm(code: &str, proxy: &str) -> String`

- [ ] **Step 1: Add the codegen module**

Create `crates/labby-codemode/src/wasm_codegen.rs`:

```rust
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
globalThis.callTool = (id, params = {{}}) => {{
  if (typeof id !== "string" || id.trim() === "") throw new TypeError("callTool id must be a non-empty string");
  if (params === null || typeof params !== "object" || Array.isArray(params)) throw new TypeError("callTool params must be a JSON object");
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitToolCall(JSON.stringify({{ id, params: __labEncodeResult(params) }}));
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
    const seq = globalThis.__labEmitSnippetResolve(JSON.stringify({{ name, input: __labEncodeResult(input) }}));
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
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})().then(
  (value) => globalThis.__labEmitDone(JSON.stringify({{ result: __labEncodeResult(value), has_result: value !== undefined }})),
  (error) => globalThis.__labEmitDone(JSON.stringify({{ error: String(error && error.message || error) }}))
);
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
    )
}

pub(crate) async fn compile_code_mode_wasm(
    plugin: &javy_codegen::Plugin,
    code: &str,
    proxy: &str,
) -> Result<Vec<u8>> {
    let source = wrap_code_mode_for_wasm(code, proxy);
    let js = JS::from_bytes(source.into_bytes());
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
    }
}
```

If `JS::from_bytes` does not exist in Javy v9, replace with the verified constructor from `javy-codegen::JS` source and update this task note in the commit body.

- [ ] **Step 2: Register modules**

Modify `crates/labby-codemode/src/lib.rs`:

```rust
mod wasm_codegen;
mod wasm_plugin;
```

- [ ] **Step 3: Run the wrapper test**

Run:

```bash
cargo test -p labby-codemode wasm_codegen --all-features
```

Expected:

```text
test result: ok.
```

- [ ] **Step 4: Add a generation smoke test after plugin loading works**

Append to `wasm_codegen.rs` tests:

```rust
#[tokio::test]
async fn generated_module_compiles_under_shared_engine() {
    let plugin = crate::wasm_plugin::load_wasm_plugin().unwrap();
    let wasm = compile_code_mode_wasm(&plugin.plugin, "async () => 2 + 2", "").await.unwrap();
    wasmtime::Module::from_binary(&plugin.engine, &wasm).unwrap();
}
```

- [ ] **Step 5: Run the generation smoke test**

Run:

```bash
cargo test -p labby-codemode generated_module_compiles_under_shared_engine --all-features -- --nocapture
```

Expected:

```text
test generated_module_compiles_under_shared_engine ... ok
```

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/lib.rs crates/labby-codemode/src/wasm_codegen.rs
git commit -m "feat(codemode): generate javy wasm modules"
```

## Task 3: Add Bounded Guest Memory Helpers Inside The Bridge

**Files:**
- Modify: `crates/labby-codemode/src/wasm_bridge.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Test: `crates/labby-codemode/src/wasm_bridge.rs`

**Interfaces:**
- Consumes: `wasmtime::Memory`, `wasmtime::StoreContext`.
- Produces:
  - `pub(crate) const CODE_MODE_WASM_BRIDGE_MAX_BYTES: usize`
  - `pub(crate) fn read_guest_string(...) -> Result<String, ToolError>`
  - `pub(crate) fn read_guest_json(...) -> Result<serde_json::Value, ToolError>`
  - `pub(crate) fn write_guest_i32_checked(...) -> Result<(), ToolError>`

**Engineering-review correction:** Keep these helpers inside `wasm_bridge.rs` until there is a second caller. Do not create `wasm_memory.rs` solely to house one consumer. The helper tests must include attacker-sized lengths, pointer overflow, OOB reads, OOB output slots, non-UTF8, malformed JSON, and cap-before-copy behavior.

- [ ] **Step 1: Add memory helper code**

Add to `crates/labby-codemode/src/wasm_bridge.rs`:

```rust
//! Bounded reads from Wasmtime guest linear memory.

use serde::de::DeserializeOwned;
use wasmtime::{AsContext, Memory};

use crate::error::ToolError;

pub(crate) const CODE_MODE_WASM_BRIDGE_MAX_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn checked_guest_bytes<C: AsContext>(
    store: C,
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

pub(crate) fn write_guest_i32_checked<C: wasmtime::AsContextMut>(
    mut store: C,
    memory: &Memory,
    ptr: i32,
    value: i32,
) -> Result<(), ToolError> {
    if ptr < 0 {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "Code Mode Wasm bridge output pointer was negative".to_string(),
        });
    }
    let ptr = ptr as usize;
    let end = ptr.checked_add(4).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "Code Mode Wasm bridge output pointer overflowed".to_string(),
    })?;
    if memory.data_size(store.as_context()) < end {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "Code Mode Wasm bridge output pointer was outside guest memory".to_string(),
        });
    }
    memory
        .write(store.as_context_mut(), ptr, &value.to_le_bytes())
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("failed to write Code Mode Wasm bridge output: {err}"),
        })
}

pub(crate) fn read_guest_string<C: AsContext>(
    store: C,
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

pub(crate) fn read_guest_json<C: AsContext, T: DeserializeOwned>(
    store: C,
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
```

- [ ] **Step 2: Register module**

Modify `crates/labby-codemode/src/lib.rs`:

```rust
mod wasm_memory;
```

- [ ] **Step 3: Add OOB, cap-before-copy, and non-UTF8 tests**

Append to `wasm_bridge.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Engine, Memory, MemoryType, Store};

    fn memory_with(bytes: &[u8]) -> (Store<()>, Memory) {
        let engine = Engine::default();
        let mut store = Store::new(&engine, ());
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
        let err = checked_guest_bytes(&store, &memory, i32::MAX, 16, CODE_MODE_WASM_BRIDGE_MAX_BYTES).unwrap_err();
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
    fn rejects_oob_output_slot() {
        let (mut store, memory) = memory_with(b"abc");
        let err = write_guest_i32_checked(&mut store, &memory, 65_535, 7).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("outside guest memory"));
    }
}
```

- [ ] **Step 4: Run memory tests**

Run:

```bash
cargo test -p labby-codemode wasm_bridge --all-features
```

Expected:

```text
test result: ok.
```

- [ ] **Step 5: Commit**

```bash
git add crates/labby-codemode/src/lib.rs crates/labby-codemode/src/wasm_bridge.rs
git commit -m "feat(codemode): bound wasm bridge memory reads"
```

## Task 4: Implement Wasmtime Bridge Over Existing Runner Protocol

**Engineering-review correction:** The code skeleton that originally appeared in this task used a synchronous `lab_bridge` request/response import. Do not implement that shape. The accepted v1 bridge preserves the native runner's pending-promise fan-out model:

```rust
// wasm_bridge.rs public surface
pub(crate) struct WasmRunState {
    pub(crate) memory: Option<wasmtime::Memory>,
    pub(crate) settle_pending: Option<wasmtime::TypedFunc<String, ()>>,
    pub(crate) done: Option<Result<crate::protocol::CodeModeRunnerResult, crate::error::ToolError>>,
}

pub(crate) fn install_lab_imports(
    linker: &mut wasmtime::Linker<WasmRunState>,
    namespace: &str,
) -> anyhow::Result<()>;

pub(crate) fn settle_pending_operation(
    store: &mut wasmtime::Store<WasmRunState>,
    input: &crate::protocol::CodeModeRunnerInput,
) -> Result<(), crate::error::ToolError>;
```

`install_lab_imports` defines these imports in the plugin namespace:

```rust
lab_emit_tool_call(ptr: i32, len: i32) -> i32
lab_emit_artifact_write(ptr: i32, len: i32) -> i32
lab_emit_snippet_resolve(ptr: i32, len: i32) -> i32
lab_emit_done(ptr: i32, len: i32) -> ()
```

Each emit import reads bounded UTF-8 JSON from guest memory, validates the payload, allocates a new runner seq, emits the corresponding existing `CodeModeRunnerOutput` line to the parent through a helper in `runner_io.rs`, and returns the seq immediately. It must not wait for the parent reply inside the import. `settle_pending_operation` serializes the existing `CodeModeRunnerInput` reply and calls the wrapper's `__labSettlePendingOperation(message)` function so the JS pending promise resolves later, preserving `Promise.all([callTool(a), callTool(b)])` overlap.

Add these tests in this task before moving on:

```rust
#[tokio::test]
async fn wasm_bridge_preserves_tool_call_fanout() {
    let started = std::time::Instant::now();
    let response = execute_code_mode_for_test(
        r#"async () => {
            const [a, b] = await Promise.all([
                callTool("test::delayed", { id: "a", delay_ms: 200 }),
                callTool("test::delayed", { id: "b", delay_ms: 200 })
            ]);
            return [a.id, b.id];
        }"#,
    )
    .await
    .unwrap();
    assert_eq!(response.result, Some(serde_json::json!(["a", "b"])));
    assert!(started.elapsed() < std::time::Duration::from_millis(350));
}

#[tokio::test]
async fn wasm_bridge_reports_final_result_through_done_import() {
    let response = execute_code_mode_for_test("async () => ({ ok: true })").await.unwrap();
    assert_eq!(response.result, Some(serde_json::json!({ "ok": true })));
}
```

Move runner-side blocking seq/read/write primitives into `runner_io.rs`, not `runner.rs`, so `runner.rs -> wasm_runner -> wasm_bridge` does not form an entrypoint-coupling knot. If old code below this correction mentions `lab_bridge`, `wait_for_bridge_response`, `runner_read_input_for_wasm_bridge`, `__lab_result_ptr`, or `__lab_result_len`, treat that text as obsolete and replace it with this correction during implementation.

**Files:**
- Create: `crates/labby-codemode/src/wasm_bridge.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: existing runner integration tests plus new bridge unit tests

**Interfaces:**
- Consumes: `CodeModeRunnerInput`, `CodeModeRunnerOutput`, `RUNNER_STATE`, `wasm_memory`.
- Produces:
  - `pub(crate) struct WasmBridge`
  - `pub(crate) fn install_bridge_imports(linker: &mut wasmtime::Linker<WasmRunState>, namespace: &str) -> anyhow::Result<()>`
  - `pub(crate) fn emit_protocol_request(...) -> Result<serde_json::Value, ToolError>`

- [ ] **Step 1: Add bridge state type**

Create `crates/labby-codemode/src/wasm_bridge.rs`:

```rust
//! Wasmtime imports that preserve the existing parent-owned runner protocol.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use wasmtime::{Caller, Linker, Memory};

use crate::error::ToolError;
use crate::protocol::{
    CodeModeRunnerInput, CodeModeRunnerOutput, RUNNER_STATE,
};
use crate::wasm_memory::{CODE_MODE_WASM_BRIDGE_MAX_BYTES, read_guest_json};

#[derive(Default)]
pub(crate) struct WasmRunState {
    pub(crate) memory: Option<Memory>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
enum BridgeRequest {
    ToolCall { id: String, params: Value },
    ArtifactWrite {
        path: String,
        content: String,
        #[serde(default)]
        content_type: Option<String>,
    },
    SnippetResolve {
        name: String,
        #[serde(default)]
        input: Value,
    },
}

pub(crate) fn install_bridge_imports(
    linker: &mut Linker<WasmRunState>,
    namespace: &str,
) -> Result<()> {
    linker.func_wrap(namespace, "lab_bridge", lab_bridge)?;
    Ok(())
}

fn lab_bridge(
    mut caller: Caller<'_, WasmRunState>,
    request_ptr: i32,
    request_len: i32,
    response_ptr_out: i32,
    response_len_out: i32,
) -> wasmtime::Result<i32> {
    let Some(memory) = caller.data().memory else {
        return Ok(bridge_error(&mut caller, response_ptr_out, response_len_out, "internal_error", "Code Mode Wasm memory was not registered")?);
    };
    let request: BridgeRequest = match read_guest_json(
        caller.as_context(),
        &memory,
        request_ptr,
        request_len,
        CODE_MODE_WASM_BRIDGE_MAX_BYTES,
    ) {
        Ok(request) => request,
        Err(err) => {
            return Ok(bridge_error(&mut caller, response_ptr_out, response_len_out, err.kind(), &err.to_string())?);
        }
    };
    let response = match emit_protocol_request(request) {
        Ok(value) => value,
        Err(err) => json!({ "type": "tool_error", "kind": err.kind(), "message": err.to_string() }),
    };
    write_bridge_response(&mut caller, &memory, response_ptr_out, response_len_out, &response)
}

fn emit_protocol_request(request: BridgeRequest) -> Result<Value, ToolError> {
    let seq = next_runner_seq()?;
    let output = match request {
        BridgeRequest::ToolCall { id, params } => CodeModeRunnerOutput::ToolCall { seq, id, params },
        BridgeRequest::ArtifactWrite { path, content, content_type } => {
            CodeModeRunnerOutput::ArtifactWrite { seq, path, content, content_type }
        }
        BridgeRequest::SnippetResolve { name, input } => {
            CodeModeRunnerOutput::SnippetResolve { seq, name, input }
        }
    };
    runner_emit(output)?;
    wait_for_bridge_response(seq)
}

fn wait_for_bridge_response(seq: u64) -> Result<Value, ToolError> {
    loop {
        let input = crate::runner::runner_read_input_for_wasm_bridge()?;
        match input {
            CodeModeRunnerInput::ToolResult { seq: got, result } if got == seq => {
                return Ok(json!({ "type": "tool_result", "result": result }));
            }
            CodeModeRunnerInput::SnippetResolved { seq: got, code, input } if got == seq => {
                return Ok(json!({ "type": "snippet_resolved", "code": code, "input": input }));
            }
            CodeModeRunnerInput::ToolError { seq: got, kind, message } if got == seq => {
                return Ok(json!({ "type": "tool_error", "kind": kind, "message": message }));
            }
            other => {
                return Err(ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("runner received unexpected response while waiting for seq {seq}: {other:?}"),
                });
            }
        }
    }
}
```

This sketch intentionally references helpers that must be made visible from `runner.rs` without changing protocol shapes:

```rust
pub(crate) fn runner_read_input_for_wasm_bridge() -> Result<CodeModeRunnerInput, ToolError>
pub(crate) fn runner_emit_for_wasm_bridge(output: CodeModeRunnerOutput) -> Result<(), ToolError>
pub(crate) fn next_runner_seq_for_wasm_bridge() -> Result<u64, ToolError>
```

Use those names if they fit the final module layout; if implementation keeps helpers private to `wasm_bridge.rs`, update imports accordingly.

- [ ] **Step 2: Add response writing**

Add to `wasm_bridge.rs`:

```rust
fn bridge_error(
    caller: &mut Caller<'_, WasmRunState>,
    response_ptr_out: i32,
    response_len_out: i32,
    kind: &str,
    message: &str,
) -> wasmtime::Result<i32> {
    let value = json!({ "type": "tool_error", "kind": kind, "message": message });
    let Some(memory) = caller.data().memory else {
        return Ok(0);
    };
    write_bridge_response(caller, &memory, response_ptr_out, response_len_out, &value)
}

fn write_bridge_response(
    caller: &mut Caller<'_, WasmRunState>,
    memory: &Memory,
    response_ptr_out: i32,
    response_len_out: i32,
    value: &Value,
) -> wasmtime::Result<i32> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > CODE_MODE_WASM_BRIDGE_MAX_BYTES {
        anyhow::bail!("Code Mode Wasm bridge response exceeded cap");
    }
    let alloc = caller
        .get_export("cabi_realloc")
        .and_then(|export| export.into_func())
        .ok_or_else(|| anyhow::anyhow!("generated module did not export cabi_realloc"))?;
    let alloc = alloc.typed::<(i32, i32, i32, i32), i32>(&caller)?;
    let ptr = alloc.call(caller.as_context_mut(), (0, 0, 1, bytes.len() as i32))?;
    memory.write(caller.as_context_mut(), ptr as usize, &bytes)?;
    memory.write(caller.as_context_mut(), response_ptr_out as usize, &ptr.to_le_bytes())?;
    memory.write(caller.as_context_mut(), response_len_out as usize, &(bytes.len() as i32).to_le_bytes())?;
    Ok(1)
}
```

If Wasmtime does not permit looking up `cabi_realloc` from this import context in the final shape, switch to returning `(ptr,len)` via two exported mutable globals or an explicit guest-exported allocation helper. Keep the bounded cap and add a regression test for the chosen mechanism.

- [ ] **Step 3: Register module**

Modify `crates/labby-codemode/src/lib.rs`:

```rust
mod wasm_bridge;
```

- [ ] **Step 4: Run focused bridge tests**

Run:

```bash
cargo test -p labby-codemode wasm_bridge --all-features -- --nocapture
```

Expected:

```text
test result: ok.
```

- [ ] **Step 5: Commit**

```bash
git add crates/labby-codemode/src/lib.rs crates/labby-codemode/src/runner.rs crates/labby-codemode/src/wasm_bridge.rs
git commit -m "feat(codemode): bridge wasm host calls over runner protocol"
```

## Task 5: Execute Generated Wasm With Fuel, Epoch, And Resource Limits

**Engineering-review correction:** This task must implement the following before any PR handoff:

```rust
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
    module_cache: lru::LruCache<[u8; 32], wasmtime::Module>,
    epoch_ticker: EpochTickerGuard,
}
```

Mandatory implementation details:

- `WasmRunner::execute` takes `timeout: Duration` in addition to `code` and `proxy`.
- Reject `code.len() + proxy.len() > MAX_SOURCE_BYTES` before Javy codegen.
- Wrap `compile_code_mode_wasm` and `Module::from_binary` in deadlines derived from the parent runner deadline; if they time out, return `kind = "timeout"` with `trap_cause = "codegen_timeout"` or `trap_cause = "module_compile_timeout"` in logs.
- Derive epoch deadline ticks from `timeout`, for example `deadline_ticks = max(1, (timeout.as_millis() / CODE_MODE_WASM_EPOCH_TICK_MS as u128).saturating_sub(1))`, so a 5 second caller timeout does not accidentally get a 30 second epoch window.
- Compute `fuel_consumed = initial_fuel - store.get_fuel()?` after successful execution and log it.
- Add a bounded per-runner LRU cache keyed by `sha256(wrapped_source + proxy + plugin_hash + javy_version + codegen_options)` and cache the compiled `wasmtime::Module`; every execution still creates a fresh `Store` and `Instance`.
- Before instantiation, inspect generated module imports and reject anything outside the allow-list: the Lab Javy plugin import namespace (`memory`, `cabi_realloc`, `invoke`) plus the Lab bridge imports (`lab_emit_tool_call`, `lab_emit_artifact_write`, `lab_emit_snippet_resolve`, `lab_emit_done`) if they appear on the plugin side. Add a test that no `wasi:*`, filesystem, env, random, clock, or socket imports are accepted from generated user modules.
- Use `linker.instance(&mut store, &plugin.import_namespace, plugin_instance)?` or the verified Wasmtime 45 equivalent to define plugin exports for the generated module. Do not use a nonexistent `linker.store_context()`.
- Delete the speculative `__lab_result_ptr` / `__lab_result_len` extraction path. Read the final result from `store.data().done`, which is set by `lab_emit_done`.
- Classify only Wasmtime fuel exhaustion and epoch interruption as caller-facing `timeout`. Missing exports, bad imports, guest OOB traps, allocation failures, bridge ABI bugs, and plugin initialization failures are `server_error` or `invalid_param` and should evict the runner unless a test proves the runner is cleanly reusable.

Add these tests:

```rust
#[tokio::test]
async fn generated_module_rejects_unexpected_imports() {
    let err = compile_and_validate_imports_for_test(r#"(module (import "wasi:filesystem" "open" (func)))"#)
        .unwrap_err();
    assert_eq!(err.kind(), "server_error");
    assert!(err.to_string().contains("unexpected Wasm import"));
}

#[tokio::test]
async fn source_size_is_rejected_before_codegen() {
    let oversized = "x".repeat(crate::config::MAX_SOURCE_BYTES + 1);
    let err = WasmRunner::new().unwrap()
        .execute(&oversized, "", std::time::Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    assert!(err.to_string().contains("source"));
}

#[tokio::test]
async fn unexpected_oob_trap_is_not_reported_as_timeout() {
    let err = execute_oob_trap_for_test().await.unwrap_err();
    assert_ne!(err.kind(), "timeout");
}
```

**Files:**
- Create: `crates/labby-codemode/src/wasm_runner.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: `crates/labby-codemode/src/wasm_runner.rs`

**Interfaces:**
- Consumes: `wasm_plugin::WasmPlugin`, `wasm_codegen::compile_code_mode_wasm`, `wasm_bridge::install_bridge_imports`.
- Produces:
  - `pub(crate) struct WasmRunner`
  - `pub(crate) fn new() -> Result<Self, CodeModeRunnerError>`
  - `pub(crate) async fn execute(&self, code: &str, proxy: &str) -> Result<CodeModeRunnerResult, CodeModeRunnerError>`

- [ ] **Step 1: Add runner skeleton**

Create `crates/labby-codemode/src/wasm_runner.rs`:

```rust
//! Wasmtime-backed execution for one Code Mode runner subprocess.

use std::sync::Arc;

use anyhow::{Context, Result};
use wasmtime::{Instance, Linker, Module, ResourceLimiter, Store};

use crate::protocol::CodeModeRunnerResult;
use crate::wasm_bridge::{WasmRunState, install_bridge_imports};
use crate::wasm_codegen::compile_code_mode_wasm;
use crate::wasm_plugin::{
    CODE_MODE_WASM_DEFAULT_FUEL, CODE_MODE_WASM_EPOCH_DEADLINE_TICKS,
    CODE_MODE_WASM_MEMORY_LIMIT_BYTES, WasmPlugin, load_wasm_plugin, wasm_limits_disabled,
};

pub(crate) struct WasmRunner {
    plugin: Arc<WasmPlugin>,
}

impl WasmRunner {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            plugin: Arc::new(load_wasm_plugin()?),
        })
    }

    pub(crate) async fn execute(&self, code: &str, proxy: &str) -> Result<CodeModeRunnerResult> {
        let wasm = compile_code_mode_wasm(&self.plugin.plugin, code, proxy).await?;
        let module = Module::from_binary(&self.plugin.engine, &wasm)
            .context("failed to compile generated Code Mode Wasm module")?;
        let mut store = Store::new(&self.plugin.engine, WasmRunState::default());
        if !wasm_limits_disabled() {
            store.set_fuel(CODE_MODE_WASM_DEFAULT_FUEL)?;
            store.set_epoch_deadline(CODE_MODE_WASM_EPOCH_DEADLINE_TICKS);
        }
        store.limiter(|_| &mut CodeModeLimiter);
        let mut linker = Linker::new(&self.plugin.engine);
        install_bridge_imports(&mut linker, &self.plugin.import_namespace)?;
        let plugin_instance = linker.instantiate(&mut store, &self.plugin.module)?;
        define_dynamic_imports(&mut linker, &self.plugin.import_namespace, plugin_instance)?;
        let instance = linker.instantiate(&mut store, &module)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("generated Code Mode module did not export memory")?;
        store.data_mut().memory = Some(memory);
        let start = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .context("generated Code Mode module did not export _start")?;
        start.call(&mut store, ())?;
        read_main_result(&mut store, &instance)
    }
}

struct CodeModeLimiter;

impl ResourceLimiter for CodeModeLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= CODE_MODE_WASM_MEMORY_LIMIT_BYTES)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(false)
    }
}
```

- [ ] **Step 2: Add dynamic import definitions**

Append to `wasm_runner.rs`:

```rust
fn define_dynamic_imports(
    linker: &mut Linker<WasmRunState>,
    namespace: &str,
    plugin_instance: Instance,
) -> Result<()> {
    let memory = plugin_instance
        .get_memory(linker.store_context(), "memory")
        .context("Javy plugin did not export memory")?;
    let cabi_realloc = plugin_instance
        .get_func(linker.store_context(), "cabi_realloc")
        .context("Javy plugin did not export cabi_realloc")?;
    let invoke = plugin_instance
        .get_func(linker.store_context(), "invoke")
        .context("Javy plugin did not export invoke")?;
    linker.define(namespace, "memory", memory)?;
    linker.define(namespace, "cabi_realloc", cabi_realloc)?;
    linker.define(namespace, "invoke", invoke)?;
    Ok(())
}
```

If `Linker` does not expose `store_context()` in Wasmtime 45, implement this helper with the verified Wasmtime 45 signature:

```rust
fn define_dynamic_imports(
    store: &mut Store<WasmRunState>,
    linker: &mut Linker<WasmRunState>,
    namespace: &str,
    plugin_instance: Instance,
) -> Result<()>
```

and use `plugin_instance.get_memory(&mut *store, "memory")`.

- [ ] **Step 3: Add result extraction**

Append to `wasm_runner.rs`:

```rust
fn read_main_result(
    store: &mut Store<WasmRunState>,
    instance: &Instance,
) -> Result<CodeModeRunnerResult> {
    let memory = store
        .data()
        .memory
        .context("Code Mode Wasm memory not registered after execution")?;
    let result_ptr = instance
        .get_global(&mut *store, "__lab_result_ptr")
        .context("generated module did not expose __lab_result_ptr")?
        .get(&mut *store)
        .i32()
        .context("__lab_result_ptr was not i32")?;
    let result_len = instance
        .get_global(&mut *store, "__lab_result_len")
        .context("generated module did not expose __lab_result_len")?
        .get(&mut *store)
        .i32()
        .context("__lab_result_len was not i32")?;
    let value: serde_json::Value = crate::wasm_memory::read_guest_json(
        &*store,
        &memory,
        result_ptr,
        result_len,
        crate::wasm_memory::CODE_MODE_WASM_BRIDGE_MAX_BYTES,
    )?;
    Ok(CodeModeRunnerResult::from_response_result(Some(value)))
}
```

If Javy's dynamic module cannot expose result globals as written, alter the JS wrapper to write the final result through `lab_bridge("done", { result })` and have `wasm_bridge` retain it in `WasmRunState`. The accepted final interface must still produce `CodeModeRunnerResult`.

- [ ] **Step 4: Add epoch ticker**

Add to `wasm_runner.rs`:

```rust
fn spawn_epoch_ticker(engine: wasmtime::Engine) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(
            crate::wasm_plugin::CODE_MODE_WASM_EPOCH_TICK_MS,
        ));
        engine.increment_epoch();
    })
}
```

Keep one ticker per runner subprocess, created in `WasmRunner::new`, or use a `OnceLock` keyed by `Engine` if the implementation can prove it does not leak threads across tests. The runner process is intentionally long-lived, so one thread for the process lifetime is acceptable.

- [ ] **Step 5: Register module**

Modify `crates/labby-codemode/src/lib.rs`:

```rust
mod wasm_runner;
```

- [ ] **Step 6: Wire runner.rs to use WasmRunner**

In `crates/labby-codemode/src/runner.rs`, replace the block that creates `javy::Config`, `javy::Runtime`, installs native callbacks, evals `wrap_code_mode`, resolves pending jobs, and emits `Done` with:

```rust
let wasm_runner = WASM_RUNNER.with(|cell| {
    let mut cell = cell.borrow_mut();
    if cell.is_none() {
        *cell = Some(crate::wasm_runner::WasmRunner::new().map_err(|err| {
            CodeModeRunnerError {
                kind: "server_error".to_string(),
                message: format!("failed to initialize Code Mode Wasm runner: {err}"),
            }
        })?);
    }
    Ok::<_, CodeModeRunnerError>(cell.as_ref().unwrap().clone())
})?;

let result = wasm_runner
    .execute(&code, &proxy)
    .await
    .map_err(|err| classify_wasm_execution_error(err))?;

runner_emit(CodeModeRunnerOutput::Done {
    result,
    logs: Vec::new(),
})
.map_err(CodeModeRunnerError::from)?;
Ok(RunnerLoopOutcome::Completed)
```

If `run_code_mode_runner` remains synchronous, introduce a small Tokio current-thread runtime in the runner subprocess:

```rust
let rt = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .map_err(|err| CodeModeRunnerError {
        kind: "server_error".to_string(),
        message: format!("failed to create Code Mode runner async runtime: {err}"),
    })?;
let result = rt.block_on(wasm_runner.execute(&code, &proxy))?;
```

Do not use async Wasmtime imports to call parent-side `CodeModeHost` directly.

- [ ] **Step 7: Add trap classification**

Add in `runner.rs` or `wasm_runner.rs`:

```rust
fn classify_wasm_execution_error(err: anyhow::Error) -> CodeModeRunnerError {
    let message = err.to_string();
    if message.contains("all fuel consumed")
        || message.contains("epoch deadline")
        || message.contains("wasm trap")
    {
        return CodeModeRunnerError {
            kind: "timeout".to_string(),
            message: format!("Code Mode execution timed out: {message}"),
        };
    }
    CodeModeRunnerError {
        kind: "server_error".to_string(),
        message: add_code_mode_hint("server_error", &message),
    }
}
```

Keep richer `trap_cause` in tracing fields, not the caller-facing kind.

- [ ] **Step 8: Run focused execution tests**

Run:

```bash
cargo test -p labby-codemode wasm_runner --all-features -- --nocapture
```

Expected:

```text
test result: ok.
```

- [ ] **Step 9: Commit**

```bash
git add crates/labby-codemode/src/lib.rs crates/labby-codemode/src/runner.rs crates/labby-codemode/src/wasm_runner.rs
git commit -m "feat(codemode): execute code mode under wasmtime"
```

## Task 6: Preserve Runner Reuse And Timeout Disposition

**Files:**
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: existing pool/runner tests

**Interfaces:**
- Consumes: existing `DriveOutcome::{Completed, ExecutionError, RunnerUnhealthy}`.
- Produces: tests proving fuel/epoch execution errors release pooled runners, while parent wall-clock timeout still evicts.

- [ ] **Step 1: Add a pooled-runner trap reuse test**

Add a test near existing runner-drive timeout tests:

```rust
#[tokio::test]
async fn wasm_timeout_error_releases_pooled_runner_for_reuse() {
    let host = TestHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let first = broker
        .run_in_runner(
            "async () => { while (true) {} }".to_string(),
            String::new(),
            Duration::from_secs(5),
            CodeModeCaller::trusted_local(),
            CodeModeSurface::Cli,
            100,
            64 * 1024,
            false,
            ToolScope::Unscoped,
        )
        .await
        .unwrap_err();
    assert_eq!(first.kind(), "timeout");

    let second = broker
        .run_in_runner(
            "async () => 42".to_string(),
            String::new(),
            Duration::from_secs(5),
            CodeModeCaller::trusted_local(),
            CodeModeSurface::Cli,
            100,
            64 * 1024,
            false,
            ToolScope::Unscoped,
        )
        .await
        .unwrap();
    assert_eq!(second.result, Some(serde_json::json!(42)));
}
```

Use the repo's existing test host constructors and type names if they differ; preserve the asserted behavior.

- [ ] **Step 2: Re-run parent wall-clock eviction test**

Run:

```bash
cargo test -p labby-codemode timeout --all-features -- --nocapture
```

Expected:

```text
test result: ok.
```

- [ ] **Step 3: Run pool tests**

Run:

```bash
cargo test -p labby-codemode pool --all-features
```

Expected:

```text
test result: ok.
```

- [ ] **Step 4: Commit**

```bash
git add crates/labby-codemode/src/runner_drive.rs crates/labby-codemode/src/runner.rs
git commit -m "test(codemode): prove wasm traps preserve runner reuse"
```

## Task 7: Run Full Code Mode Parity Tests

**Files:**
- Modify: existing tests under `crates/labby-codemode/src/` as needed
- Test: `crates/labby-codemode`

**Interfaces:**
- Consumes: Wasmtime runner and bridge.
- Produces: parity evidence for JS API compatibility.

- [ ] **Step 1: Run the existing crate suite**

Run:

```bash
cargo nextest run -p labby-codemode --all-features
```

Expected:

```text
PASS 162 tests
```

The exact count may increase after new tests. All tests must pass.

- [ ] **Step 2: Add explicit structured rejection parity if missing**

Add a test:

```rust
#[tokio::test]
async fn tool_error_rejection_remains_json_parse_recoverable() {
    let response = execute_code_mode_for_test(
        r#"async () => {
            try {
                await callTool("missing::tool", {});
            } catch (e) {
                return JSON.parse(e.message);
            }
        }"#,
    )
    .await
    .unwrap();
    assert_eq!(response.result.unwrap()["kind"], "unknown_tool");
}
```

Use the existing helper name in the test module. If the exact expected kind is `server_error` or a local test stub kind, assert the repo's current parity kind.

- [ ] **Step 3: Add artifact path traversal parity if missing**

Add a test:

```rust
#[tokio::test]
async fn wasm_sourced_artifact_path_traversal_is_rejected() {
    let err = execute_code_mode_for_test(
        r#"async () => {
            await writeArtifact("../../etc/passwd", "nope");
        }"#,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    assert!(err.to_string().contains("path"));
}
```

- [ ] **Step 4: Add snippet budget parity if missing**

Add a test that resolves snippets until the existing max count is exceeded:

```rust
#[tokio::test]
async fn wasm_snippet_resolve_budget_matches_existing_contract() {
    let err = execute_code_mode_for_test(
        r#"async () => {
            for (let i = 0; i < 64; i++) {
                await codemode.run("tiny", {});
            }
        }"#,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "snippet_resolve_limit");
}
```

- [ ] **Step 5: Re-run the full crate suite**

Run:

```bash
cargo nextest run -p labby-codemode --all-features
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src
git commit -m "test(codemode): preserve wasm runner api parity"
```

## Task 8: Update Docs And Observability

**Files:**
- Modify: `crates/labby-codemode/CLAUDE.md`
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/dev/ERRORS.md`
- Modify: `docs/dev/OBSERVABILITY.md`
- Modify: `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`

**Interfaces:**
- Consumes: final implemented module names and trap behavior.
- Produces: docs that no longer contradict implementation.

- [ ] **Step 1: Update crate runtime docs**

In `crates/labby-codemode/CLAUDE.md`, replace:

```markdown
## Runtime — Javy/QuickJS via subprocess stdio (NOT Wasmtime)
```

with:

```markdown
## Runtime — Javy-generated Wasm under Wasmtime inside subprocess stdio
```

Add:

```markdown
The live Code Mode runner compiles wrapped caller JS with Bytecode Alliance Javy dynamic codegen and executes the generated module under Wasmtime inside the existing runner subprocess. The subprocess pool, env isolation, process-group/Job Object kill behavior, PR_SET_DUMPABLE, per-execution cwd jail, and parent-owned stdio protocol remain the outer containment and authority boundary.

Caller-facing timeout errors continue to use `kind = "timeout"`. Internal logs distinguish `trap_cause = "fuel_exhausted"`, `trap_cause = "epoch_interrupted"`, and `trap_cause = "os_subprocess_timeout"` when available.
```

- [ ] **Step 2: Update file responsibility table**

Add rows:

```markdown
| `wasm_plugin.rs` | Javy plugin bytes, Wasmtime engine/module setup, and Wasm limit constants. |
| `wasm_codegen.rs` | Wraps Code Mode JS and compiles it into a Javy dynamic-linking Wasm module. |
| `wasm_memory.rs` | Bounded guest linear-memory reads and UTF-8/JSON decoding helpers. |
| `wasm_bridge.rs` | Wasmtime imports that emit/wait on the existing parent/runner protocol for tool/artifact/snippet authority. |
| `wasm_runner.rs` | Per-Start Store/Instance execution, fuel, epoch, memory limits, and trap classification. |
```

- [ ] **Step 3: Update errors doc**

In `docs/dev/ERRORS.md`, document:

```markdown
`timeout` remains the stable caller-facing kind for Code Mode execution timeout. This includes parent wall-clock expiry, Wasmtime fuel exhaustion, and Wasmtime epoch interruption. Operators should use structured logs' `trap_cause` field to distinguish the internal cause.
```

- [ ] **Step 4: Update observability doc**

In `docs/dev/OBSERVABILITY.md`, add Code Mode-specific fields:

```markdown
Code Mode Wasmtime runner events may include `runtime = "wasmtime"`, `trap_cause`, `fuel_remaining`, and `wasm_compile_ms`. These fields are diagnostic only; standard dispatch fields remain `surface`, `service`, `action`, `elapsed_ms`, and `kind`.
```

- [ ] **Step 5: Run stale language search with Lumen first, then exact text checks**

Run Lumen semantic search for stale Code Mode Wasmtime prohibitions:

```text
query: "Do not reintroduce Wasmtime fuel code_mode_fuel_exhausted NOT Wasmtime"
path: /home/jmagar/workspace/lab/.worktrees/codemode-wasmtime-runtime-implementation
```

Then run exact checks:

```bash
git grep -n "Do not reintroduce Wasmtime\\|NOT Wasmtime\\|code_mode_fuel_exhausted" -- crates/labby-codemode docs || true
```

Expected: no stale prohibition remains except intentional historical references in the spike doc.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/CLAUDE.md docs/dev/CODE_MODE.md docs/dev/ERRORS.md docs/dev/OBSERVABILITY.md docs/dev/CODE_MODE_WASMTIME_SPIKE.md
git commit -m "docs(codemode): describe wasmtime runtime"
```

## Task 9: Full Verification And PR Handoff

**Files:**
- All touched files

**Interfaces:**
- Consumes: completed implementation.
- Produces: green worktree, pushed branch, PR update.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --all
```

Expected: command exits 0.

- [ ] **Step 2: Check focused crate**

Run:

```bash
cargo check -p labby-codemode --all-features
```

Expected: command exits 0.

- [ ] **Step 3: Test focused crate**

Run:

```bash
cargo nextest run -p labby-codemode --all-features
```

Expected: all tests pass.

- [ ] **Step 4: Run mandatory workspace build gate**

Run:

```bash
cargo build --workspace --all-features
```

Expected: command exits 0.

- [ ] **Step 5: Run lint gate**

Run:

```bash
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all --check
```

Expected: both commands exit 0.

- [ ] **Step 6: Run dependency gates**

Run:

```bash
cargo deny check
cargo audit
```

Expected: `cargo deny check` passes. `cargo audit` has no new Wasmtime/WASI 42 advisories; any remaining findings are the existing baseline and must be listed in the PR body.

- [ ] **Step 7: Confirm no Wasmtime/WASI 42 packages**

Run:

```bash
cargo tree -i wasmtime@42.0.2 || true
cargo tree -i wasmtime-wasi@42.0.2 || true
```

Expected:

```text
error: package ID specification `wasmtime@42.0.2` did not match any packages
error: package ID specification `wasmtime-wasi@42.0.2` did not match any packages
```

- [ ] **Step 8: Confirm native Javy hot path is removed**

Run Lumen semantic search first for stale native Javy runner usage:

```text
query: "javy Runtime Config Function new __labEmitToolCall native QuickJS hot path"
path: /home/jmagar/workspace/lab/.worktrees/codemode-wasmtime-runtime-implementation/crates/labby-codemode
```

Then run exact checks:

```bash
git grep -n "javy::Runtime\\|javy::Config\\|javy::quickjs\\|__labEmitToolCall.*Function::new" -- crates/labby-codemode/src crates/labby-codemode/Cargo.toml || true
cargo tree -p labby-codemode -i javy || true
```

Expected: no native `javy::Runtime`/`javy::Config` hot-path usage remains. If the `javy` crate is no longer needed, remove it from `crates/labby-codemode/Cargo.toml`. If it remains for tests or helper types, document exactly why in the PR body.

- [ ] **Step 9: Run latency and fuel benchmark gate**

Run the benchmark command added by the implementation, for example:

```bash
cargo run -p labby --all-features -- internal code-mode-benchmark --samples 20 --json > /tmp/codemode-wasmtime-benchmark.json
```

Expected JSON contains:

```json
{
  "warm_trivial_p95_ms": 0,
  "baseline_native_p95_ms": 0,
  "overhead_ratio": 0,
  "wrap_ms": 0,
  "javy_codegen_ms": 0,
  "wasm_module_compile_ms": 0,
  "plugin_instantiate_ms": 0,
  "generated_instantiate_ms": 0,
  "bridge_roundtrip_ms": 0,
  "fuel_consumed_p99": 0,
  "chosen_fuel_budget": 0
}
```

Acceptance: warm trivial p95 must be explicitly judged against the current native baseline captured before removal. If p95 overhead is greater than 2x or greater than 25ms absolute, stop and escalate before creating a ready PR. Include raw benchmark JSON or a checked-in summarized table under `docs/dev/`.

- [ ] **Step 10: Commit remaining changes**

```bash
git status --short
git add .
git commit -m "feat(codemode): run code mode through wasmtime"
```

If there are no changes, skip the commit and record that all task commits already captured the work.

- [ ] **Step 11: Push branch**

```bash
git push -u origin HEAD
```

Expected: branch pushed.

- [ ] **Step 12: Create or update PR**

If continuing PR #174, run:

```bash
gh pr edit 174 --body-file /tmp/codemode-wasmtime-pr-body.md
gh pr ready 174
```

If creating a new implementation PR, run:

```bash
gh pr create --draft --title "Implement Code Mode Wasmtime runtime" --body-file /tmp/codemode-wasmtime-pr-body.md
```

PR body must include:

```markdown
## Summary
- replaces native QuickJS execution inside the runner subprocess with Javy-generated Wasm under Wasmtime
- preserves parent-owned stdio host authority for tools, artifacts, and snippets
- keeps caller-facing timeout kind stable while adding internal trap diagnostics

## Verification
- cargo check -p labby-codemode --all-features
- cargo nextest run -p labby-codemode --all-features
- cargo build --workspace --all-features
- cargo clippy --workspace --all-features -- -D warnings
- cargo deny check
- cargo audit (baseline advisories only; no Wasmtime/WASI 42)
- cargo tree -i wasmtime@42.0.2 || true
- cargo tree -i wasmtime-wasi@42.0.2 || true
- native Javy hot-path grep/tree check
- Code Mode Wasmtime latency/fuel benchmark summary
```

## Self-Review

Spec coverage:
- Dependency unblock is covered in Task 1 and Task 9.
- Plugin artifact path is covered in Task 1.
- Javy dynamic codegen is covered in Task 2.
- Guest-memory validation is covered in Task 3.
- Parent-owned stdio authority is covered in Task 4.
- Wasmtime fuel, epoch, memory, and trap mapping are covered in Task 5.
- Runner reuse after fuel/epoch traps is covered in Task 6.
- API parity is covered in Task 7.
- Docs and observability are covered in Task 8.
- Full verification and PR handoff are covered in Task 9.

Placeholder scan:
- No `TBD`, `TODO`, `implement later`, or "similar to" placeholders remain.
- Some snippets are marked as sketches where the final Wasmtime 45 signature may differ; each includes the exact fallback signature and the required test behavior.

Type consistency:
- `WasmPlugin`, `compile_code_mode_wasm`, `WasmRunState`, and `WasmRunner` are introduced before later tasks consume them.
- `CodeModeRunnerInput` and `CodeModeRunnerOutput` are the existing protocol types and are not renamed.
- Caller-facing `timeout` behavior is consistent across tasks and docs.
