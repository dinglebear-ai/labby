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
    context.authenticated_principal = Some(subject.into());
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
        let gateway_config = config.to_gateway_config();
        let manager=GatewayManager::with_store(home.join("config.toml"),GatewayRuntimeHandle::default(), Arc::new(labby::dispatch::gateway::config_store::LabConfigStore::new(Arc::new(std::sync::RwLock::new(config)),home.join("config.toml"))))
            .with_step_journal(Arc::new(store)).with_code_mode_runner_spawn(RunnerSpawn {program:env!("CARGO_BIN_EXE_labby").into(),args:vec!["internal".into(),"code-mode-runner".into()]});
        manager.seed_config(gateway_config).await;
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

#[test]
fn declared_snippet_metadata_survives_disabled_synthetic_advertisement() {
    if let Ok(home) = std::env::var("LABBY_DECLARED_PREVIEW_CHILD") {
        declared_metadata_contract(Path::new(&home));
        return;
    }
    for enabled in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "declared_snippet_metadata_survives_disabled_synthetic_advertisement",
                "--nocapture",
            ])
            .env("LABBY_DECLARED_PREVIEW_CHILD", home.path())
            .env("LABBY_PREVIEW_CODE_MODE_ENABLED", enabled.to_string())
            .env("LABBY_HOME", home.path())
            .env("HOME", home.path())
            .env("LABBY_MCP_HTTP_PORT", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "enabled={enabled}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn declared_metadata_contract(home: &Path) {
    fs::create_dir_all(home.join("snippets")).unwrap();
    fs::write(home.join("snippets/declared.md"), "---\nname: declared\ndescription: Declared metadata\ntools: [\"fixture::echo\"]\n---\n```js\nasync () => ({ ok: true })\n```\n").unwrap();
    let script = home.join("metadata.py");
    fs::write(&script, r"import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    method = request['method']
    if method == 'initialize':
        result = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'snippet-metadata-fixture', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'description': 'Synthetic echo', 'inputSchema': {'type': 'object', 'properties': {'value': {'type': 'string'}}}, 'outputSchema': {'type': 'object', 'properties': {'ok': {'type': 'boolean'}}}, 'annotations': {'readOnlyHint': True}}]}
    elif method == 'ping':
        result = {}
    elif method == 'tools/call':
        raise RuntimeError('metadata must never invoke a tool')
    else:
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'Method not found'}}), flush=True)
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
").unwrap();
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap().trim().to_owned();
    let enabled = std::env::var("LABBY_PREVIEW_CODE_MODE_ENABLED").unwrap();
    let text = format!(
        "[code_mode]\nenabled = {enabled}\n[gateway]\nextra_stdio_commands = [{}]\n[[upstream]]\nname = \"fixture\"\ntransport = \"stdio\"\ncommand = {}\nargs = [{}]\n",
        serde_json::to_string(&python).unwrap(),
        serde_json::to_string(&python).unwrap(),
        serde_json::to_string(script.to_str().unwrap()).unwrap()
    );
    let config: labby::config::LabConfig = toml::from_str(&text).unwrap();
    let gateway_config = config.to_gateway_config();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let manager = GatewayManager::with_store(
                home.join("config.toml"),
                GatewayRuntimeHandle::default(),
                Arc::new(labby::dispatch::gateway::config_store::LabConfigStore::new(
                    Arc::new(std::sync::RwLock::new(config)),
                    home.join("config.toml"),
                )),
            );
            manager.seed_config(gateway_config).await;
            let caller = SnippetDispatchContext::trusted_local();
            let preview = call(
                &manager,
                "snippets.preview",
                json!({"name":"declared"}),
                &caller,
            )
            .await
            .unwrap();
            assert_eq!(preview["can_execute"], true, "{preview}");
            assert_eq!(
                preview["declared_tools"][0]["status"], "allowed",
                "{preview}"
            );
            let fixture = call(
                &manager,
                "snippets.fixture",
                json!({"name":"declared"}),
                &caller,
            )
            .await
            .unwrap();
            assert!(fixture.to_string().contains("fixture::echo"), "{fixture}");
        });
}

#[test]
fn contended_snippet_publication_does_not_block_async_worker() {
    if let Ok(home) = std::env::var("LABBY_PUBLICATION_TEST_CHILD") {
        let home = Path::new(&home);
        fs::create_dir_all(home.join("snippets")).unwrap();
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join("snippets/.snippet-write.lock"))
            .unwrap();
        lock.lock().unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let started = std::time::Instant::now();
                let publish = labby::dispatch::snippets::dispatch(
                    "snippets.create",
                    json!({"name":"contended","body":"async () => ({\"ok\":true})"}),
                );
                let heartbeat = async {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    assert!(
                        started.elapsed() < std::time::Duration::from_millis(500),
                        "contended storage monopolized the async worker for {:?}",
                        started.elapsed()
                    );
                };
                let (result, ()) = tokio::join!(publish, heartbeat);
                assert!(
                    result.is_err(),
                    "the held publication lock must remain authoritative"
                );
                assert!(!home.join("snippets/contended.md").exists());
            });
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "contended_snippet_publication_does_not_block_async_worker",
            "--nocapture",
        ])
        .env("LABBY_PUBLICATION_TEST_CHILD", home.path())
        .env("LABBY_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
