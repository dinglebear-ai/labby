//! Automatic, owner-bound preservation of oversized JSON outputs.
use crate::artifacts::{
    CodeModeArtifactReceipt, CodeModeArtifactWrite, artifact_max_bytes, write_code_mode_artifact,
};
use crate::{CodeModeCaller, ToolScope};
use serde_json::Value;
use std::path::Path;

/// Zero disables automatic response storage; explicit writeArtifact is unaffected.
pub(crate) fn inline_threshold() -> usize {
    std::env::var("LABBY_CODE_MODE_AUTO_ARTIFACT_THRESHOLD_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(24 * 1024)
}

pub(crate) async fn preserve(
    root: &Path,
    path: String,
    value: &Value,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Option<CodeModeArtifactReceipt> {
    // Automatic persistence must never create inaccessible records or broaden authority.
    if inline_threshold() == 0
        || scope.is_read_only()
        || crate::artifact_access::owner(caller, scope).is_err()
    {
        return None;
    }
    let operation = async {
        let content = serde_json::to_string(value).ok()?;
        if content.len() > artifact_max_bytes() {
            return None;
        }
        let mut receipt = write_code_mode_artifact(
            root,
            &CodeModeArtifactWrite {
                path,
                content,
                content_type: Some("application/json".into()),
            },
            artifact_max_bytes(),
        )
        .await
        .ok()?;
        crate::artifact_access::enroll(root, &mut receipt, caller, scope)
            .await
            .ok()?;
        Some(receipt)
    };
    // Persistence is best effort and must not indefinitely delay response delivery.
    tokio::time::timeout(std::time::Duration::from_millis(500), operation)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn automatic_preservation_respects_read_only_and_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(ulid::Ulid::new().to_string());
        let value = serde_json::json!({"items":[1,2,3]});
        assert!(
            preserve(
                &root,
                "automatic/result.json".into(),
                &value,
                &CodeModeCaller::TrustedLocal,
                &ToolScope::default().read_only()
            )
            .await
            .is_none()
        );
        assert!(!root.exists());
        let receipt = preserve(
            &root,
            "automatic/result.json".into(),
            &value,
            &CodeModeCaller::TrustedLocal,
            &ToolScope::default(),
        )
        .await
        .unwrap();
        assert!(receipt.artifact_id.is_some());
        let saved = tokio::fs::read_to_string(root.join("automatic/result.json"))
            .await
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&saved).unwrap(), value);
    }
}
