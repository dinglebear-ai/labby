//! Development mockup routes.
//!
//! Filesystem discovery and reads run on a bounded blocking pool so large
//! prototype directories cannot stall API runtime workers.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

static IO_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

fn directory() -> std::path::PathBuf {
    crate::config::home_dir()
        .map(|home| home.join(".superpowers/brainstorm/content"))
        .unwrap_or_else(|| std::path::PathBuf::from(".superpowers/brainstorm/content"))
}

fn newest_in(directory: &std::path::Path, fragment: Option<&str>) -> Option<std::path::PathBuf> {
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("html"))
        .filter(|entry| {
            fragment.is_none_or(|needle| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_some_and(|stem| stem.contains(needle))
            })
        })
        .filter_map(|entry| {
            entry
                .metadata()
                .ok()?
                .modified()
                .ok()
                .map(|modified| (entry.path(), modified))
        })
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

fn response_blocking(directory: &std::path::Path, fragment: Option<&str>) -> Response {
    match newest_in(directory, fragment) {
        None => {
            let escaped = fragment
                .map(|name| {
                    format!(
                        " '{}'",
                        name.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                            .replace('"', "&quot;")
                    )
                })
                .unwrap_or_default();
            Html(format!("<p style='font-family:sans-serif;padding:2rem'>No{escaped} mockup found in <code>~/.superpowers/brainstorm/content/</code></p>")).into_response()
        }
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(html) => Html(html).into_response(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "failed to read dev mockup");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
    }
}

async fn response(fragment: Option<String>) -> Response {
    let permit = match IO_PERMITS.acquire().await {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let response =
        tokio::task::spawn_blocking(move || response_blocking(&directory(), fragment.as_deref()))
            .await
            .unwrap_or_else(|error| {
                tracing::error!(%error, "dev mockup blocking task failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            });
    drop(permit);
    response
}

pub(super) async fn dev_mockup() -> Response {
    response(None).await
}

pub(super) async fn dev_mockup_named(Path(name): Path<String>) -> Response {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    response(Some(name)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn large_directory_reads_are_bounded_and_responsive() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..2_000 {
            std::fs::write(
                directory.path().join(format!("mock-{index:04}.html")),
                index.to_string(),
            )
            .unwrap();
        }
        let mut reads = Vec::new();
        for _ in 0..16 {
            let directory = directory.path().to_path_buf();
            reads.push(tokio::task::spawn_blocking(move || {
                response_blocking(&directory, Some("mock"))
            }));
        }
        tokio::time::timeout(Duration::from_millis(250), tokio::task::yield_now())
            .await
            .unwrap();
        for read in reads {
            assert_eq!(read.await.unwrap().status(), StatusCode::OK);
        }
    }
}
