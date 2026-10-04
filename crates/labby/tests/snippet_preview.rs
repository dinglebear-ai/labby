//! Authoritative metadata preview and guarded rerun through real product dispatch.
#![cfg(feature = "gateway")]
use labby::dispatch::snippets::dispatch::{
    SnippetDispatchContext, dispatch_with_manager_and_context,
};
use labby_codemode::{CodeModeCaller, CodeModeCallerCapabilities, CodeModeSurface, RunnerSpawn};
use labby_gateway::{
    codemode_journal::StepJournalStore,
    gateway::manager::{GatewayManager, GatewayRuntimeHandle},
};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Arc};

fn context(subject: &str) -> SnippetDispatchContext {
    let mut context = SnippetDispatchContext::trusted_local();
    context.execution_surface = CodeModeSurface::Api;
    context.actor_key = Some("preview-operator".into());
    context.execution_caller = CodeModeCaller::Scoped {
        sub: Some(subject.into()),
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: true,
            is_admin: true,
        },
    };
    context
}
fn source(code: &str) -> String {
    format!(
        "---\nname: preview-demo\ndescription: Guard regression\ntools: []\ninputs:\n  secret:\n    type: string\n    required: true\n---\n```js\n{code}\n```\n"
    )
}
fn call<'a>(
    manager: &'a GatewayManager,
    action: &'a str,
    params: Value,
    caller: &SnippetDispatchContext,
) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, labby_runtime::error::ToolError>> + 'a>> {
    Box::pin(dispatch_with_manager_and_context(
        manager,
        action,
        params,
        Some(caller.clone()),
    ))
}
fn count(home: &Path) -> i64 {
    rusqlite::Connection::open(home.join("journal.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM snippet_receipts", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn preview_is_read_only_and_replay_requires_current_owner_guard_and_all_drift() {
    if let Ok(home) = std::env::var("LABBY_PREVIEW_TEST_CHILD") {
        run(Path::new(&home));
        return;
    }
    // The advertisement switch must not disable native snippet preview/replay.
    // Run the complete durability and denial regression under both MCP regimes.
    for enabled in [true, false] {
        let home = if cfg!(target_os = "macos") {
            tempfile::Builder::new()
                .prefix("lpv-")
                .tempdir_in("/private/tmp")
        } else {
            tempfile::tempdir()
        }
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "preview_is_read_only_and_replay_requires_current_owner_guard_and_all_drift",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path())
            .env("LABBY_PREVIEW_TEST_CHILD", home.path())
            .env("LABBY_PREVIEW_CODE_MODE_ENABLED", enabled.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run(home: &Path) {
    fs::create_dir(home.join("snippets")).unwrap();
    let path = home.join("snippets/preview-demo.md");
    fs::write(
        &path,
        source("async () => { throw new Error('must not evaluate during metadata preview'); }"),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store=StepJournalStore::open(home.join("journal.db")).await.unwrap();
        let mut config = labby::config::LabConfig::default();
        config.code_mode.enabled = std::env::var("LABBY_PREVIEW_CODE_MODE_ENABLED").unwrap().parse().unwrap();
        let manager=GatewayManager::with_store(home.join("config.toml"),GatewayRuntimeHandle::default(), Arc::new(labby::dispatch::gateway::config_store::LabConfigStore::new(Arc::new(std::sync::RwLock::new(config)),home.join("config.toml"))))
            .with_step_journal(Arc::new(store)).with_code_mode_runner_spawn(RunnerSpawn {program:env!("CARGO_BIN_EXE_labby").into(),args:vec!["internal".into(),"code-mode-runner".into()]});
        let alice=context("alice");
        let secret="ephemeral-preview-secret-must-not-be-stored-90b76";
        let input=json!({"secret":secret});
        let preview=call(&manager,"snippets.preview",json!({"name":"preview-demo","params":input}),&alice).await.unwrap();
        assert_eq!(preview["mode"],"metadata");assert_eq!(preview["dynamic_unknown"],true);assert_eq!(preview["can_execute"],true);
        assert!(!preview.to_string().contains(secret));assert_eq!(count(home),0,"preview cannot start an execution");
        fs::write(&path,source("async (input) => ({ok:true, length:input.secret.length})")).unwrap();
        let stale=call(&manager,"snippets.exec",json!({"name":"preview-demo","params":input,"expected_preview_fingerprint":preview["preview_fingerprint"]}),&alice).await.unwrap_err();
        assert_eq!(stale.kind(),"preview_stale");assert_eq!(count(home),0);
        let preview=call(&manager,"snippets.preview",json!({"name":"preview-demo","params":input}),&alice).await.unwrap();
        let changed_input=call(&manager,"snippets.exec",json!({"name":"preview-demo","params":{"secret":"changed before execution"},"expected_preview_fingerprint":preview["preview_fingerprint"]}),&alice).await.unwrap_err();
        assert_eq!(changed_input.kind(),"preview_stale");assert_eq!(count(home),0);
        let mut changed_scope=alice.clone();changed_scope.execution_scope=changed_scope.execution_scope.read_only();
        let changed_authority=call(&manager,"snippets.exec",json!({"name":"preview-demo","params":input,"expected_preview_fingerprint":preview["preview_fingerprint"]}),&changed_scope).await.unwrap_err();
        assert_eq!(changed_authority.kind(),"preview_stale");assert_eq!(count(home),0);
        let original_config=manager.code_mode_config().await;
        let mut changed_config=original_config.clone();changed_config.timeout_ms+=1000;
        Box::pin(manager.set_code_mode_config(changed_config,None,None)).await.unwrap();
        let changed_runtime=call(&manager,"snippets.exec",json!({"name":"preview-demo","params":input,"expected_preview_fingerprint":preview["preview_fingerprint"]}),&alice).await.unwrap_err();
        assert_eq!(changed_runtime.kind(),"preview_stale");assert_eq!(count(home),0);
        Box::pin(manager.set_code_mode_config(original_config,None,None)).await.unwrap();
        let executed=call(&manager,"snippets.exec",json!({"name":"preview-demo","params":input,"expected_preview_fingerprint":preview["preview_fingerprint"]}),&alice).await.unwrap();
        let id=executed["execution_id"].as_str().unwrap();assert_eq!(count(home),1);
        let receipt=call(&manager,"snippets.receipt",json!({"execution_id":id}),&alice).await.unwrap();
        assert_eq!(receipt["tool_schema_digests"],json!({}));assert!(!receipt.to_string().contains(secret));
        let replay=call(&manager,"snippets.preview",json!({"execution_id":id,"params":input}),&alice).await.unwrap();
        assert_eq!(replay["drift"],json!([]));
        assert_eq!(call(&manager,"snippets.preview",json!({"execution_id":id,"params":input}),&context("bob")).await.unwrap_err().kind(),"unknown_execution");
        let mut nonadmin=alice.clone();nonadmin.is_admin=false;
        assert_eq!(call(&manager,"snippets.preview",json!({"name":"preview-demo","params":input}),&nonadmin).await.unwrap_err().kind(),"forbidden");
        let mut caller_nonadmin=alice.clone();caller_nonadmin.execution_caller=CodeModeCaller::Scoped {sub:Some("alice".into()),capabilities:CodeModeCallerCapabilities {can_read:true,can_execute:true,can_use_snippets:true,is_admin:false}};
        assert_eq!(call(&manager,"snippets.preview",json!({"name":"preview-demo","params":input}),&caller_nonadmin).await.unwrap_err().kind(),"forbidden");
        let wrong=call(&manager,"snippets.replay",json!({"execution_id":id,"params":input,"expected_preview_fingerprint":"old","acknowledged_drift":[]}),&alice).await.unwrap_err();
        assert_eq!(wrong.kind(),"preview_stale");assert_eq!(count(home),1);
        let changed=json!({"secret":"new explicit input"});
        let preview=call(&manager,"snippets.preview",json!({"execution_id":id,"params":changed}),&alice).await.unwrap();
        assert_eq!(preview["drift"],json!([{ "field":"input","status":"changed" }]));
        let missing=call(&manager,"snippets.replay",json!({"execution_id":id,"params":changed,"expected_preview_fingerprint":preview["preview_fingerprint"],"acknowledged_drift":[]}),&alice).await.unwrap_err();
        assert_eq!(missing.kind(),"confirmation_required");assert_eq!(count(home),1);
        let rerun=call(&manager,"snippets.replay",json!({"execution_id":id,"params":changed,"expected_preview_fingerprint":preview["preview_fingerprint"],"acknowledged_drift":["input"]}),&alice).await.unwrap();
        assert_ne!(rerun["execution_id"],executed["execution_id"]);assert_eq!(rerun["result"]["length"],18);assert_eq!(count(home),2);
        // Simulate an older receipt: absent schema evidence must prompt explicitly.
        rusqlite::Connection::open(home.join("journal.db")).unwrap().execute("UPDATE snippet_receipts SET receipt_json=json_remove(receipt_json,'$.tool_schema_digests') WHERE execution_id=?1",[id]).unwrap();
        let old=call(&manager,"snippets.preview",json!({"execution_id":id,"params":input}),&alice).await.unwrap();
        assert_eq!(old["drift"],json!([{ "field":"tool_schema","status":"unverifiable" }]));
        assert_eq!(call(&manager,"snippets.replay",json!({"execution_id":id,"params":input,"expected_preview_fingerprint":old["preview_fingerprint"],"acknowledged_drift":[]}),&alice).await.unwrap_err().kind(),"confirmation_required");
        assert_eq!(count(home),2);
        call(&manager,"snippets.replay",json!({"execution_id":id,"params":input,"expected_preview_fingerprint":old["preview_fingerprint"],"acknowledged_drift":["tool_schema"]}),&alice).await.unwrap();
        assert_eq!(count(home),3);
    });
    for filename in ["journal.db", "journal.db-wal", "journal.db-shm"] {
        if let Ok(bytes) = fs::read(home.join(filename)) {
            assert!(
                !bytes
                    .windows(b"ephemeral-preview-secret-must-not-be-stored-90b76".len())
                    .any(|bytes| bytes == b"ephemeral-preview-secret-must-not-be-stored-90b76")
            );
        }
    }
}
