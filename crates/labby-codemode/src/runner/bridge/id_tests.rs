use super::*;

#[test]
fn native_bridge_rejects_oversized_ids_before_protocol_emission() {
    let runtime = javy::Runtime::new(javy::Config::default()).expect("test runtime");
    runtime.context().with(|cx| -> javy::quickjs::Result<()> {
        cx.globals().set(
            "emit", javy::quickjs::Function::new(
                cx.clone(), javy::quickjs::prelude::MutFn::new(|cx, args| {
                    javy_emit_tool_call(javy::Args::hold(cx, args))
                }),
            )?,
        )?;
        for id in [
            "x".repeat(crate::protocol::MAX_TOOL_ID_BYTES + 1),
            "é".repeat(crate::protocol::MAX_TOOL_ID_BYTES / 2 + 1),
        ] {
            cx.globals().set("id", id.clone())?;
            // No RUNNER_STATE is initialized: reaching emission would fail
            // differently. The ID guard must run before seq/params allocation.
            let message: String = cx.eval("(() => { try { emit(id, {}); return 'missing rejection'; } catch (error) { return error.message; } })()")?;
            let error: CodeModeCallError = serde_json::from_str(&message).expect("typed bridge error");
            assert_eq!(error.kind(), "budget_exceeded");
            assert_eq!(error.origin, crate::CodeModeErrorOrigin::Budget);
            assert!(error.tool.is_none());
            assert!(!message.contains(&id));
        }
        Ok(())
    }).expect("bridge validation");
}
