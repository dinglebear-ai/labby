//! Protocol regressions: every catchable rejection is acknowledged before Done.

use super::*;
use crate::protocol::CodeModeRunnerInput;

fn request_script(requests: &[CodeModeRunnerOutput], capture: &Path) -> String {
    let mut script = String::from("IFS= read -r _\n");
    for request in requests {
        let json = serde_json::to_string(request).expect("request JSON");
        script.push_str(&format!(
            "printf '%s\\n' '{}'\nIFS= read -r reply || exit 21\nprintf '%s\\n' \"$reply\" >>\"{}\"\n",
            json.replace('\'', "'\"'\"'"), capture.display(),
        ));
    }
    script.push_str("printf '%s\\n' '{\"type\":\"done\"}'\nIFS= read -r _\n");
    script
}

fn replies(capture: &Path) -> Vec<CodeModeRunnerInput> {
    std::fs::read_to_string(capture)
        .expect("wire capture")
        .lines()
        .map(|line| serde_json::from_str(line).expect("typed wire reply"))
        .collect()
}

#[tokio::test]
async fn retrieval_after_internal_ceiling_rejects_every_promise_with_a_budget_error() {
    let mut requests: Vec<_> = (0..MAX_INTERNAL_CALLS_PER_RUN as u64)
        .map(|seq| CodeModeRunnerOutput::ToolCall {
            seq,
            id: "__lab_internal::semantic_rank".into(),
            params: json!({"query":"q","limit":1}),
        })
        .collect();
    let retrievals = [
        "read_artifact",
        "artifact_info",
        "list_artifacts",
        "list_resources",
        "read_resource",
        "get_prompt",
        "list_skills",
        "get_skill",
    ];
    for (index, name) in retrievals.iter().enumerate() {
        requests.push(CodeModeRunnerOutput::ToolCall {
            seq: (MAX_INTERNAL_CALLS_PER_RUN + index) as u64,
            id: format!("__lab_internal::{name}"),
            params: json!({}),
        });
    }
    let capture = tempfile::NamedTempFile::new().expect("capture");
    let mut runner = PooledRunner::spawn_stub_script(&request_script(&requests, capture.path()))
        .expect("protocol stub");
    let host = NoopHost::default();
    let config = test_config(Duration::from_secs(15));
    let outcome = CodeModeBroker::new(Some(&host))
        .drive_runner(
            &mut runner,
            &config,
            tokio::time::Instant::now() + config.timeout,
        )
        .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("every internal promise must settle")
    };
    assert!(response.calls.is_empty());
    let replies = replies(capture.path());
    assert_eq!(replies.len(), requests.len());
    for (index, reply) in replies.iter().enumerate().skip(MAX_INTERNAL_CALLS_PER_RUN) {
        let CodeModeRunnerInput::ToolError { seq, error } = reply else {
            panic!("retrieval must reject, not invent empty data")
        };
        assert_eq!(*seq, index as u64);
        assert_eq!(error.kind(), "internal_call_budget_exceeded");
        assert_eq!(error.origin, crate::CodeModeErrorOrigin::Budget);
    }
}

#[tokio::test]
async fn host_rejects_oversized_ids_without_echoing_or_retaining_them() {
    let ids = [
        "x".repeat(crate::protocol::MAX_TOOL_ID_BYTES + 1),
        "é".repeat(crate::protocol::MAX_TOOL_ID_BYTES / 2 + 1),
    ];
    let requests: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(seq, id)| CodeModeRunnerOutput::ToolCall {
            seq: seq as u64,
            id: id.clone(),
            params: json!({}),
        })
        .collect();
    let capture = tempfile::NamedTempFile::new().expect("capture");
    let mut runner = PooledRunner::spawn_stub_script(&request_script(&requests, capture.path()))
        .expect("protocol stub");
    let host = NoopHost::default();
    let config = test_config(Duration::from_secs(5));
    let outcome = CodeModeBroker::new(Some(&host))
        .drive_runner(
            &mut runner,
            &config,
            tokio::time::Instant::now() + config.timeout,
        )
        .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("oversized IDs must be catchable")
    };
    assert_eq!(response.calls.len(), ids.len());
    assert!(
        response
            .calls
            .iter()
            .all(|call| call.id == protocol_boundary::OVERSIZED_TOOL_CALL_ID
                && call.params.is_none()
                && call.error_kind.as_deref() == Some("budget_exceeded"))
    );
    let wire = std::fs::read_to_string(capture.path()).expect("capture");
    let trace = serde_json::to_string(&response.calls).expect("trace JSON");
    for id in &ids {
        assert!(!wire.contains(id));
        assert!(!trace.contains(id));
    }
    assert_eq!(
        replies(capture.path()).len(),
        ids.len(),
        "every rejected seq acknowledged before Done"
    );
}

#[tokio::test]
async fn repeated_budget_rejections_stop_at_trace_capacity_without_an_ack_stall() {
    let capture = tempfile::NamedTempFile::new().expect("capture");
    let script = format!(
        r#"
IFS= read -r _
i=0
while [ "$i" -le {limit} ]; do
  printf '{{"type":"tool_call","seq":%d,"id":"fixture::rejected","params":{{}}}}\n' "$i"
  IFS= read -r reply || exit 21
  printf '%s\n' "$reply" >>"{capture}"
  i=$((i+1))
done
printf '%s\n' '{{"type":"done"}}'
IFS= read -r _
"#,
        limit = host_trace::MAX_HOST_TRACE_ROWS,
        capture = capture.path().display()
    );
    let mut runner = PooledRunner::spawn_stub_script(&script).expect("protocol stub");
    let host = NoopHost::default();
    let config = test_config(Duration::from_secs(30));
    let outcome = CodeModeBroker::new(Some(&host))
        .drive_runner(
            &mut runner,
            &config,
            tokio::time::Instant::now() + config.timeout,
        )
        .await;
    let DriveOutcome::RunnerUnhealthy(error) = outcome else {
        panic!("trace overflow must fail explicitly")
    };
    assert_eq!(error.kind(), "trace_budget_exceeded");
    assert_eq!(error.calls().len(), host_trace::MAX_HOST_TRACE_ROWS);
    assert_eq!(
        replies(capture.path()).len(),
        error.calls().len(),
        "all retained/rejected requests were acknowledged; overflow ends execution"
    );
    assert!(
        error
            .calls()
            .iter()
            .any(|call| call.error_kind.as_deref() == Some("call_budget_exceeded"))
    );
    assert!(runner.child.try_wait().expect("child status").is_some());
}

#[tokio::test]
async fn non_admin_writable_runner_cannot_publish_metadata_path_aliases() {
    use crate::types::CodeModeCallerCapabilities;

    let aliases = [
        "./.labby-artifact-metadata/payload",
        "././.labby-artifact-metadata/payload",
        r".\.labby-artifact-metadata\payload",
        "./.LABBY-ARTIFACT-METADATA/payload",
        "./.Labby-Artifact-Metadata/payload",
    ];
    let requests: Vec<_> = aliases
        .iter()
        .enumerate()
        .map(|(seq, path)| CodeModeRunnerOutput::ArtifactWrite {
            seq: seq as u64,
            path: (*path).into(),
            content: "payload".into(),
            content_type: None,
        })
        .collect();
    let capture = tempfile::NamedTempFile::new().expect("capture");
    let mut runner = PooledRunner::spawn_stub_script(&request_script(&requests, capture.path()))
        .expect("protocol stub");
    let host = NoopHost::default();
    let mut config = test_config(Duration::from_secs(5));
    config.caller = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("metadata-alias-regression".into()),
    };
    assert!(config.caller.can_execute());
    assert!(!config.caller.is_admin());
    let root = code_mode_artifact_root(&config.artifact_run_id);
    let outcome = CodeModeBroker::new(Some(&host))
        .drive_runner(
            &mut runner,
            &config,
            tokio::time::Instant::now() + config.timeout,
        )
        .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("reserved-path errors must be catchable")
    };
    assert_eq!(response.calls.len(), aliases.len());
    assert!(response.artifacts.is_empty());
    assert!(
        !root.exists(),
        "all aliases reject before mkdir/admission/publication"
    );
    let replies = replies(capture.path());
    assert_eq!(replies.len(), aliases.len());
    for reply in replies {
        let CodeModeRunnerInput::ToolError { error, .. } = reply else {
            panic!("reserved alias must reject")
        };
        assert_eq!(
            error.kind(),
            "invalid_param",
            "caller has execute authority; this is path rejection, not an admin gate"
        );
    }
}
