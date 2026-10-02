//! CLI adapter for the server-owned local browser handoff.
use anyhow::{Result, bail};
use serde::Deserialize;
use std::{
    net::IpAddr,
    process::{Command, Stdio},
    time::Duration,
};

fn local_server(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value)?;
    let host = url
        .host_str()
        .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok());
    if url.scheme() != "http"
        || !host.is_some_and(|host| host.is_loopback())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!("browser handoff requires the selected direct loopback HTTP server");
    }
    Ok(url)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Started {
    token: String,
    origin: String,
    expires_in_seconds: u64,
}

pub(super) async fn open(server: &str, bearer: &str) -> Result<()> {
    let mut server = local_server(server)?;
    let origin = server.origin().ascii_serialization();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?;
    server.set_path("/auth/setup-handoff/start");
    let mut started = None;
    for attempt in 0..4 {
        let response = client.post(server.clone()).bearer_auth(bearer).send().await;
        if let Ok(response) = response {
            if response.status().is_success() {
                let bytes = response.bytes().await?;
                if bytes.len() > 1024 {
                    bail!("invalid local handoff response");
                }
                started = Some(
                    serde_json::from_slice::<Started>(&bytes)
                        .map_err(|_| anyhow::anyhow!("invalid local handoff response"))?,
                );
                break;
            }
            // Never retry an authorization/transport refusal or expose response bodies.
            if response.status().is_client_error() {
                bail!("local browser handoff was refused");
            }
        }
        if attempt < 3 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let started =
        started.ok_or_else(|| anyhow::anyhow!("local browser handoff server is unavailable"))?;
    if started.origin != origin
        || started.expires_in_seconds != 60
        || started.token.len() != 43
        || !started
            .token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("invalid local handoff response");
    }
    server.set_path("/settings/");
    server.set_fragment(Some(&format!("setup_handoff={}", started.token)));
    let status = browser_command(server.as_str()).status()?;
    if !status.success() {
        bail!("browser launcher did not open local setup");
    }
    Ok(())
}
/// Best-effort browser launch; always retain the printed manual URL.
pub(super) fn open_url(url: &str) {
    if !browser_command(url)
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("Open this URL in your browser: {url}");
    }
}

fn browser_command(url: &str) -> Command {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let mut command = Command::new("false");
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bearer_handoff_never_follows_remote_or_ambiguous_targets() {
        assert!(local_server("http://127.0.0.1:8765").is_ok());
        assert!(local_server("http://[::1]:8765/").is_ok());
        for url in [
            "https://example.org/",
            "http://example.org/",
            "http://127.0.0.1@evil.test/",
            "http://127.0.0.1:8765/path",
            "http://127.0.0.1:8765/?x=1",
            "http://localhost:8765/",
        ] {
            assert!(local_server(url).is_err());
        }
    }
}
