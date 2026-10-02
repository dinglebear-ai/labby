use super::*;
use crate::CodeModeCallerCapabilities;
use crate::artifacts::{CodeModeArtifactWrite, write_code_mode_artifact};

fn caller(sub: &str) -> CodeModeCaller {
    CodeModeCaller::Scoped {
        sub: Some(sub.into()),
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: true,
            is_admin: true,
        },
    }
}

async fn fixture(store: &Path, sub: &str) -> CodeModeArtifactReceipt {
    let root = store.join(Ulid::new().to_string());
    let mut receipt = write_code_mode_artifact(
        &root,
        &CodeModeArtifactWrite {
            path: "report.json".into(),
            content: "{\"count\":3}".into(),
            content_type: Some("application/json".into()),
        },
        1024,
    )
    .await
    .expect("write fixture");
    enroll(&root, &mut receipt, &caller(sub), &ToolScope::default())
        .await
        .expect("enroll fixture");
    receipt
}

#[tokio::test]
async fn artifact_access_owner_read_metadata_and_listing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let receipt = fixture(temp.path(), "alice").await;
    let id = receipt.artifact_id.expect("retrieval id");
    let params = json!({"artifact_id":id});
    let scope = ToolScope::default().read_only();
    let result = dispatch_at(
        temp.path(),
        "read_artifact",
        &params,
        &caller("alice"),
        &scope,
    )
    .await
    .expect("read across executions");
    assert_eq!(result["content"], "{\"count\":3}");
    assert!(result["metadata"].get("absolute_path").is_none());
    assert!(
        dispatch_at(
            temp.path(),
            "read_artifact",
            &params,
            &caller("bob"),
            &scope
        )
        .await
        .is_err()
    );
    let info = dispatch_at(
        temp.path(),
        "artifact_info",
        &params,
        &caller("alice"),
        &scope,
    )
    .await
    .expect("metadata");
    assert_eq!(info["bytes"], 11);
    let listed = dispatch_at(
        temp.path(),
        "list_artifacts",
        &json!({}),
        &caller("bob"),
        &scope,
    )
    .await
    .expect("list other caller");
    assert_eq!(listed["artifacts"], json!([]));
    let listed = dispatch_at(
        temp.path(),
        "list_artifacts",
        &json!({}),
        &caller("alice"),
        &scope,
    )
    .await
    .expect("list owner");
    assert_eq!(listed["artifacts"][0]["artifact_id"], id);
}

#[tokio::test]
async fn artifact_access_rejects_scopes_tampering_and_legacy_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let receipt = fixture(temp.path(), "alice").await;
    let id = receipt.artifact_id.expect("id");
    let params = json!({"artifact_id":id});
    let restricted = ToolScope::scoped_namespaces(vec!["team".into()], vec![]);
    assert!(
        dispatch_at(
            temp.path(),
            "read_artifact",
            &params,
            &caller("alice"),
            &restricted
        )
        .await
        .is_err()
    );
    let (root, _) = location(temp.path(), &id).expect("location");
    tokio::fs::write(root.join("report.json"), "corrupt")
        .await
        .expect("tamper fixture");
    assert!(
        dispatch_at(
            temp.path(),
            "read_artifact",
            &params,
            &caller("alice"),
            &ToolScope::default()
        )
        .await
        .is_err()
    );
    assert!(
        dispatch_at(
            temp.path(),
            "read_artifact",
            &json!({"artifact_id":"../../etc/passwd"}),
            &caller("alice"),
            &ToolScope::default()
        )
        .await
        .is_err()
    );
    tokio::fs::remove_dir_all(root)
        .await
        .expect("simulate pruning");
    assert!(
        dispatch_at(
            temp.path(),
            "artifact_info",
            &params,
            &caller("alice"),
            &ToolScope::default()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn artifact_access_pages_owner_records_without_duplicates() {
    let temp = tempfile::tempdir().expect("tempdir");
    fixture(temp.path(), "alice").await;
    fixture(temp.path(), "bob").await;
    fixture(temp.path(), "alice").await;
    let scope = ToolScope::default();
    let first = dispatch_at(
        temp.path(),
        "list_artifacts",
        &json!({"limit":1}),
        &caller("alice"),
        &scope,
    )
    .await
    .expect("first");
    assert!(first["next_cursor"].is_string());
    let second = dispatch_at(
        temp.path(),
        "list_artifacts",
        &json!({"limit":1,"cursor":first["next_cursor"]}),
        &caller("alice"),
        &scope,
    )
    .await
    .expect("second");
    assert_ne!(
        first["artifacts"][0]["artifact_id"],
        second["artifacts"][0]["artifact_id"]
    );
    assert!(second["next_cursor"].is_null());
}

#[tokio::test]
async fn artifact_access_chunks_preserve_utf8_and_reject_reserved_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(Ulid::new().to_string());
    let scope = ToolScope::default();
    let mut receipt = write_code_mode_artifact(
        &root,
        &CodeModeArtifactWrite {
            path: "unicode.txt".into(),
            content: "aéz".into(),
            content_type: None,
        },
        1024,
    )
    .await
    .expect("write unicode");
    enroll(&root, &mut receipt, &caller("alice"), &scope)
        .await
        .expect("enroll");
    let id = receipt.artifact_id.expect("id");
    let first = dispatch_at(
        temp.path(),
        "read_artifact",
        &json!({"artifact_id":id,"length":2}),
        &caller("alice"),
        &scope,
    )
    .await
    .expect("first chunk");
    assert_eq!(first["content"], "a");
    assert_eq!(first["next_offset"], 1);
    let second = dispatch_at(
        temp.path(),
        "read_artifact",
        &json!({"artifact_id":id,"offset":1,"length":3}),
        &caller("alice"),
        &scope,
    )
    .await
    .expect("second chunk");
    assert_eq!(second["content"], "éz");
    assert_eq!(second["done"], true);
    assert!(
        dispatch_at(
            temp.path(),
            "read_artifact",
            &json!({"artifact_id":id,"offset":2}),
            &caller("alice"),
            &scope
        )
        .await
        .is_err()
    );
    assert!(
        write_code_mode_artifact(
            &root,
            &CodeModeArtifactWrite {
                path: format!("{METADATA_DIR}/forged.json"),
                content: "fake".into(),
                content_type: None,
            },
            1024
        )
        .await
        .is_err()
    );
}
