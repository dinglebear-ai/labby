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
        // The final result shares the runner's root. Never overwrite an
        // explicit artifact that already owns this automatic-output path.
        if tokio::fs::try_exists(root.join(&path)).await.ok()? {
            return None;
        }
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
            &crate::artifact_access::retention_owner(caller, scope, root),
        )
        .await
        .inspect_err(|error| {
            tracing::debug!(kind = error.kind(), "automatic artifact publication failed")
        })
        .ok()?;
        // Publication is no-clobber, so this path belongs to this write. A
        // cancelled or failed enrollment removes only the newly written file.
        let mut pending = tempfile::TempPath::try_from_path(root.join(&receipt.path)).ok()?;
        // Make cancellation at this boundary deterministic in the enrollment test.
        #[cfg(test)]
        tokio::task::yield_now().await;
        if let Err(error) = crate::artifact_access::enroll(root, &mut receipt, caller, scope).await
        {
            tracing::warn!(kind = error.kind(), "automatic artifact enrollment failed");
            return None;
        }
        pending.disable_cleanup(true);
        Some(receipt)
    };
    // Persistence is best effort and must not indefinitely delay response delivery.
    tokio::time::timeout(std::time::Duration::from_millis(500), operation)
        .await
        .inspect_err(|_| tracing::debug!("automatic artifact preservation timed out"))
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn automatic_payload_is_removed_if_enrollment_fails_or_is_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let scope = ToolScope::default();
        let value = serde_json::json!(1);
        let caller = CodeModeCaller::TrustedLocal;
        let relative = "automatic/result.json";

        // A valid on-disk directory with no final name cannot supply the
        // run-id component required for enrollment. Publication can proceed.
        let child = temp.path().join("run/child");
        std::fs::create_dir_all(&child).unwrap();
        let unnamed = child.join("..");
        assert!(unnamed.file_name().is_none());
        let metadata = unnamed.join(crate::artifact_access::METADATA_DIR);
        std::fs::create_dir(&metadata).unwrap();
        std::fs::write(
            metadata.join(crate::artifact_access::retention_owner(
                &caller, &scope, &unnamed,
            )),
            [],
        )
        .unwrap();
        assert!(
            preserve(&unnamed, relative.into(), &value, &caller, &scope)
                .await
                .is_none()
        );
        assert!(unnamed.join("automatic").is_dir());
        assert!(!unnamed.join(relative).exists());

        let root = temp.path().join(ulid::Ulid::new().to_string());
        let payload = root.join(relative);
        let mut operation = Box::pin(preserve(&root, relative.into(), &value, &caller, &scope));
        // Stop polling at the first suspension after publication. Dropping
        // this future exercises the same cleanup as the preservation timeout.
        std::future::poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            if payload.exists() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
        drop(operation);
        assert!(!payload.exists());
    }

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
        assert!(
            preserve(
                &root,
                "automatic/result.json".into(),
                &serde_json::json!({"different": true}),
                &CodeModeCaller::TrustedLocal,
                &ToolScope::default(),
            )
            .await
            .is_none()
        );
        assert_eq!(
            tokio::fs::read_to_string(root.join("automatic/result.json"))
                .await
                .unwrap(),
            saved
        );
    }
}
