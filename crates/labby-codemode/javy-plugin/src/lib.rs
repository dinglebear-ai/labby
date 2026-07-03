use javy_plugin_api::{
    Config, import_namespace,
    javy::{Runtime, quickjs::prelude::Func},
};

import_namespace!("labby-codemode-plugin-v1");

#[link(wasm_import_module = "labby-codemode-plugin-v1")]
unsafe extern "C" {
    fn lab_emit_tool_call(ptr: i32, len: i32) -> i32;
    fn lab_emit_artifact_write(ptr: i32, len: i32) -> i32;
    fn lab_emit_snippet_resolve(ptr: i32, len: i32) -> i32;
    fn lab_emit_done(ptr: i32, len: i32);
    fn lab_pending_input_len() -> i32;
    fn lab_pending_input_copy(ptr: i32, len: i32);
    fn lab_console_log(ptr: i32, len: i32);
}

fn config() -> Config {
    let mut config = Config::default();
    config
        .event_loop(true)
        .text_encoding(true)
        .javy_stream_io(false);
    config
}

fn modify_runtime(runtime: Runtime) -> Runtime {
    runtime.context().with(|ctx| {
        ctx.globals()
            .set(
                "__labEmitToolCall",
                Func::from(|payload: String| -> i32 {
                    unsafe { lab_emit_tool_call(payload.as_ptr() as i32, payload.len() as i32) }
                }),
            )
            .unwrap();
        ctx.globals()
            .set(
                "__labEmitArtifactWrite",
                Func::from(|payload: String| -> i32 {
                    unsafe {
                        lab_emit_artifact_write(payload.as_ptr() as i32, payload.len() as i32)
                    }
                }),
            )
            .unwrap();
        ctx.globals()
            .set(
                "__labEmitSnippetResolve",
                Func::from(|payload: String| -> i32 {
                    unsafe {
                        lab_emit_snippet_resolve(payload.as_ptr() as i32, payload.len() as i32)
                    }
                }),
            )
            .unwrap();
        ctx.globals()
            .set(
                "__labEmitDone",
                Func::from(|payload: String| {
                    unsafe { lab_emit_done(payload.as_ptr() as i32, payload.len() as i32) }
                }),
            )
            .unwrap();
        ctx.globals()
            .set(
                "__labReadPendingInput",
                Func::from(|| -> String {
                    let len = unsafe { lab_pending_input_len() };
                    if len <= 0 {
                        return String::new();
                    }
                    let mut bytes = vec![0_u8; len as usize];
                    unsafe { lab_pending_input_copy(bytes.as_mut_ptr() as i32, len) };
                    String::from_utf8(bytes).unwrap()
                }),
            )
            .unwrap();
        ctx.globals()
            .set(
                "__labConsoleLog",
                Func::from(|payload: String| {
                    unsafe { lab_console_log(payload.as_ptr() as i32, payload.len() as i32) }
                }),
            )
            .unwrap();
    });
    runtime
}

#[unsafe(export_name = "initialize-runtime")]
fn initialize_runtime() {
    javy_plugin_api::initialize_runtime(config, modify_runtime).unwrap();
}
