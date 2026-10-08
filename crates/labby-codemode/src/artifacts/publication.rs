//! Publish complete artifacts without exposing partial files or replacing outputs.

use std::future::Future;
use std::path::Path;

use tokio::io::AsyncWriteExt;

use crate::error::ToolError;

pub(super) async fn publish(destination: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    publish_with(destination, |mut file| async move {
        file.write_all(bytes).await?;
        file.flush().await?;
        Ok(file)
    })
    .await
}

async fn publish_with<F, Fut>(destination: &Path, write: F) -> Result<(), ToolError>
where
    F: FnOnce(tokio::fs::File) -> Fut,
    Fut: Future<Output = Result<tokio::fs::File, std::io::Error>>,
{
    let parent = destination
        .parent()
        .ok_or_else(|| io_error(std::io::Error::other("artifact has no parent")))?;
    // The guard removes the temporary file on errors and cancellation. Use the
    // destination directory so publication never crosses filesystem boundaries.
    let pending = tempfile::Builder::new()
        .prefix(".labby-artifact-")
        .tempfile_in(parent)
        .map_err(io_error)?;
    let file = tokio::fs::File::from_std(pending.as_file().try_clone().map_err(io_error)?);
    let file = write(file).await.map_err(io_error)?;
    // Await outstanding filesystem work and close the cloned handle before
    // publication (also required for Windows rename/link behavior).
    drop(file.into_std().await);
    pending
        .persist_noclobber(destination)
        .map_err(|error| io_error(error.error))?;
    Ok(())
}

fn io_error(error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: if error.kind() == std::io::ErrorKind::AlreadyExists {
            "invalid_param"
        } else {
            "internal_error"
        }
        .into(),
        message: format!("failed to publish artifact file: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn failed_partial_write_leaves_path_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("output.txt");
        let error = publish_with(&destination, |mut file| async move {
            file.write_all(b"partial").await?;
            file.flush().await?;
            Err(std::io::Error::other("injected write failure"))
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "internal_error");
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        publish(&destination, b"complete").await.unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"complete");
    }

    #[tokio::test]
    async fn cancelled_partial_write_never_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("output.txt");
        let started = Arc::new(Notify::new());
        let notified = started.clone();
        let path = destination.clone();
        let task = tokio::spawn(async move {
            publish_with(&path, |mut file| async move {
                file.write_all(b"partial").await?;
                file.flush().await?;
                notified.notify_one();
                std::future::pending::<()>().await;
                Ok(file)
            })
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        assert!(!destination.exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        publish(&destination, b"retry").await.unwrap();
    }

    #[tokio::test]
    async fn publication_does_not_replace_an_existing_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("output.txt");
        publish(&destination, b"original").await.unwrap();
        assert_eq!(
            publish(&destination, b"replacement")
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
