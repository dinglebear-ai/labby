//! Saved dispatch -> real QuickJS -> durable scoped receipt, in one isolated process.
#![cfg(feature = "gateway")]
use std::{fs, path::Path, sync::Arc};

use labby::dispatch::snippets::dispatch::{
    SnippetDispatchContext, dispatch_with_manager_and_context as gateway_dispatch,
};
use labby_codemode::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeSurface, RunnerSpawn, ToolScope,
};
use labby_gateway::{
    codemode_journal::StepJournalStore,
    gateway::manager::{GatewayManager, GatewayRuntimeHandle},
};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SECRET: &str = "receipt-raw-params-must-never-persist-7fd82";
fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
fn source(code: &str) -> String {
    format!(
        "---\nname: receipt-demo\ndescription: Receipt test\ntools: []\ninputs:\n  secret:\n    type: string\n    required: true\n---\n```js\n{code}\n```\n"
    )
}
fn scoped(subject: &str) -> SnippetDispatchContext {
    let mut context = SnippetDispatchContext::trusted_local();
    context.actor_key = Some("operator".into());
    context.route_scope = "team-a".into();
    context.execution_surface = CodeModeSurface::Api;
    context.is_admin = true;
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
async fn new_manager(home: &Path) -> GatewayManager {
    let store = StepJournalStore::open(home.join("journal.db"))
        .await
        .unwrap();
    GatewayManager::new(home.join("config.toml"), GatewayRuntimeHandle::default())
        .with_step_journal(Arc::new(store))
        .with_code_mode_runner_spawn(RunnerSpawn {
            program: env!("CARGO_BIN_EXE_labby").into(),
            args: vec!["internal".into(), "code-mode-runner".into()],
        })
}
fn dispatch_scoped<'a>(
    manager: &'a GatewayManager,
    action: &'a str,
    params: Value,
    context: Option<SnippetDispatchContext>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, labby_runtime::error::ToolError>> + 'a>> {
    Box::pin(gateway_dispatch(manager, action, params, context))
}

async fn call(
    manager: &GatewayManager,
    action: &str,
    params: Value,
    context: &SnippetDispatchContext,
) -> Value {
    dispatch_scoped(manager, action, params, Some(context.clone()))
        .await
        .unwrap()
}
#[test]
fn saved_execution_receipts_survive_restart_and_enforce_subject_route_and_scope() {
    if let Ok(home) = std::env::var("LABBY_RECEIPT_TEST_CHILD") {
        run_receipt_contract(Path::new(&home));
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "saved_execution_receipts_survive_restart_and_enforce_subject_route_and_scope",
            "--nocapture",
        ])
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("LABBY_RECEIPT_TEST_CHILD", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "receipt child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_receipt_contract(home: &Path) {
    fs::create_dir(home.join("snippets")).unwrap();
    let source_body = source(
        "async (input) => { const end = Date.now()+250; while (Date.now()<end) {} return {ok:true, value:input.secret.length}; }",
    );
    fs::write(home.join("snippets/receipt-demo.md"), &source_body).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let manager = new_manager(home).await;
        let alice = scoped("alice");
        let input = json!({"secret":SECRET});
        let observe_started = async {
            let conn = rusqlite::Connection::open(home.join("journal.db")).unwrap();
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                let receipt: Option<String> = conn.query_row(
                    "SELECT receipt_json FROM snippet_receipts WHERE json_extract(receipt_json,'$.status')='started' LIMIT 1",
                    [], |row| row.get(0),
                ).optional().unwrap();
                if let Some(receipt) = receipt { return Some(serde_json::from_str::<Value>(&receipt).unwrap()); }
            }
            None
        };
        let (response, started_receipt) = tokio::join!(
            call(&manager, "snippets.exec", json!({"name":"receipt-demo","params":input}), &alice),
            observe_started,
        );
        let started_receipt = started_receipt.expect("execution never published its started receipt");
        assert_eq!(started_receipt["status"], "started");
        assert_eq!(started_receipt["execution_id"], response["execution_id"]);
        assert_eq!(response["receipt_status"], "persisted", "{response}");
        let id = response["execution_id"].as_str().unwrap();
        assert!(id.starts_with("snippet_"));
        let receipt = call(
            &manager,
            "snippets.receipt",
            json!({"execution_id":id}),
            &alice,
        )
        .await;
        assert_eq!(receipt["status"], "succeeded");
        assert_eq!(receipt["snippet_digest"], digest(source_body.as_bytes()));
        assert_eq!(
            receipt["input_digest"],
            digest(&serde_json::to_vec(&input).unwrap())
        );
        assert_eq!(
            receipt["result_digest"],
            digest(&serde_json::to_vec(&response["result"]).unwrap())
        );
        assert_eq!(receipt["tool_calls"], 0);
        assert_eq!(
            receipt["effective_scope_fingerprint"],
            ToolScope::scoped_namespaces(vec![], vec![]).fingerprint()
        );
        assert_eq!(receipt["surface"], "api");
        assert!(!serde_json::to_string(&receipt).unwrap().contains(SECRET));
        let mut nonadmin = alice.clone();
        nonadmin.is_admin = false;
        let denied = dispatch_scoped(
            &manager,
            "snippets.receipt",
            json!({"execution_id":id}),
            Some(nonadmin),
        )
        .await
        .unwrap_err();
        assert_eq!(denied.kind(), "forbidden");
        let mut different_route = alice.clone();
        different_route.route_scope = "team-b".into();
        let mut different_capability = alice.clone();
        different_capability.capability_filter_fingerprint = "different".into();
        for denied in [
            scoped("bob"),
            different_route,
            different_capability,
            SnippetDispatchContext::trusted_local(),
        ] {
            let error = dispatch_scoped(
                &manager,
                "snippets.receipt",
                json!({"execution_id":id}),
                Some(denied),
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind(), "unknown_execution");
        }
        let id = id.to_owned();
        drop(manager);
        let reopened = new_manager(home).await;
        assert_eq!(
            call(
                &reopened,
                "snippets.receipt",
                json!({"execution_id":id}),
                &alice
            )
            .await,
            receipt
        );
        // A semantically failed result still produces a completed failure receipt.
        fs::write(
            home.join("snippets/receipt-demo.md"),
            super_source_failed(),
        )
        .unwrap();
        let failed = call(
            &reopened,
            "snippets.exec",
            json!({"name":"receipt-demo","params":input}),
            &alice,
        )
        .await;
        let failure_receipt = call(
            &reopened,
            "snippets.receipt",
            json!({"execution_id":failed["execution_id"]}),
            &alice,
        )
        .await;
        assert_eq!(failure_receipt["status"], "failed");
        assert_eq!(failed["receipt_status"], "persisted");
        fs::write(
            home.join("snippets/receipt-demo.md"),
            source("async () => { throw new Error('synthetic execution failure'); }"),
        )
        .unwrap();
        let execution_error = dispatch_scoped(
            &reopened,
            "snippets.exec",
            json!({"name":"receipt-demo","params":input}),
            Some(alice.clone()),
        )
        .await
        .unwrap_err();
        let failure_metadata = execution_error.extra_fields();
        assert_eq!(failure_metadata["receipt_status"], "persisted");
        let thrown_receipt = call(
            &reopened,
            "snippets.receipt",
            json!({"execution_id":failure_metadata["execution_id"]}),
            &alice,
        )
        .await;
        assert_eq!(thrown_receipt["status"], "failed");
        assert_eq!(thrown_receipt["error_kind"], execution_error.kind());
        fs::write(
            home.join("snippets/receipt-demo.md"),
            super_source_failed(),
        )
        .unwrap();
        // Trusted local execution uses its own distinct persisted authority identity.
        let local = SnippetDispatchContext::trusted_local();
        let local_result = call(
            &reopened,
            "snippets.exec",
            json!({"name":"receipt-demo","params":input}),
            &local,
        )
        .await;
        assert_eq!(
            call(
                &reopened,
                "snippets.receipt",
                json!({"execution_id":local_result["execution_id"]}),
                &local
            )
            .await["surface"],
            "cli"
        );
        let mut unsubjected = local.clone();
        unsubjected.execution_caller = CodeModeCaller::Scoped {
            sub: None,
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: true,
                is_admin: true,
            },
        };
        let denied = dispatch_scoped(
            &reopened,
            "snippets.receipt",
            json!({"execution_id":local_result["execution_id"]}),
            Some(unsubjected.clone()),
        )
        .await
        .unwrap_err();
        assert_eq!(denied.kind(), "unknown_execution");
        let history = call(&reopened,"snippets.history",json!({"name":"receipt-demo","limit":1}),&alice).await;
        assert_eq!(history["receipt_status"],"persisted");
        assert_eq!(history["receipts"].as_array().unwrap().len(),1);
        assert!(dispatch_scoped(&reopened,"snippets.history",json!({"limit":51}),Some(alice.clone())).await.is_err());
        fs::write(home.join("snippets/artifact-demo.md"),source("async () => { await writeArtifact('report.txt', 'receipt-backed download', {contentType:'text/plain'}); return {ok:true}; }").replace("name: receipt-demo","name: artifact-demo")).unwrap();
        let run=call(&reopened,"snippets.exec",json!({"name":"artifact-demo","params":{"secret":SECRET}}),&alice).await;
        let id=run["execution_id"].as_str().unwrap();
        let artifact=call(&reopened,"snippets.artifact",json!({"execution_id":id,"path":"report.txt"}),&alice).await;
        assert_eq!(artifact["content_base64"],"cmVjZWlwdC1iYWNrZWQgZG93bmxvYWQ=");
        assert_eq!(artifact["bytes"],23);
        for denied in [scoped("bob"),unsubjected] {
            assert_eq!(dispatch_scoped(&reopened,"snippets.artifact",json!({"execution_id":id,"path":"report.txt"}),Some(denied)).await.unwrap_err().kind(),"unknown_execution");
        }
        assert_eq!(dispatch_scoped(&reopened,"snippets.artifact",json!({"execution_id":id,"path":"../journal.db"}),Some(alice.clone())).await.unwrap_err().kind(),"artifact_unavailable");
        let own_history=call(&reopened,"snippets.history",json!({"name":"artifact-demo"}),&alice).await;
        assert_eq!(own_history["receipts"].as_array().unwrap().len(),1);
        assert!(call(&reopened,"snippets.history",json!({"name":"artifact-demo"}),&scoped("bob")).await["receipts"].as_array().unwrap().is_empty());
        drop(reopened);
    });
    drop(runtime);
    for name in ["journal.db", "journal.db-wal", "journal.db-shm"] {
        if let Ok(bytes) = fs::read(home.join(name)) {
            assert!(
                !bytes
                    .windows(SECRET.len())
                    .any(|slice| slice == SECRET.as_bytes()),
                "raw parameter leaked into {name}"
            );
        }
    }
}
fn super_source_failed() -> String {
    source("async () => ({ok:false, reason:'expected fixture failure'})")
}
