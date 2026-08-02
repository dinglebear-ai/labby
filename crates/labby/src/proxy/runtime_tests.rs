//! Focused tests for the local proxy runtime validation.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::proxy::command::ProxyCommand;
use crate::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPreferences};
use crate::proxy::runtime::{LocalProxy, LocalProxyOptions};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_proxy_rejects_tailscale_exposure() {
        let command = ProxyCommand {
            program: OsString::from("echo"),
            args: vec![OsString::from("hello")],
            cwd: PathBuf::from("/tmp"),
            display: "echo hello".to_string(),
        };

        let prefs = ProxyPreferences {
            exposure: ProxyExposure::Tailscale,
            ..Default::default()
        };

        let result = LocalProxy::start(LocalProxyOptions {
            command,
            preferences: prefs,
            bearer_token: None,
            explicit_env: vec![],
            inherit_env: vec![],
        })
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }

    #[tokio::test]
    async fn local_proxy_rejects_oauth_auth() {
        let command = ProxyCommand {
            program: OsString::from("echo"),
            args: vec![OsString::from("hello")],
            cwd: PathBuf::from("/tmp"),
            display: "echo hello".to_string(),
        };

        let prefs = ProxyPreferences {
            exposure: ProxyExposure::Local,
            auth: ProxyAuthMode::Oauth,
            ..Default::default()
        };

        let result = LocalProxy::start(LocalProxyOptions {
            command,
            preferences: prefs,
            bearer_token: None,
            explicit_env: vec![],
            inherit_env: vec![],
        })
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }

    #[tokio::test]
    async fn local_proxy_rejects_tailnet_auth() {
        let command = ProxyCommand {
            program: OsString::from("echo"),
            args: vec![OsString::from("hello")],
            cwd: PathBuf::from("/tmp"),
            display: "echo hello".to_string(),
        };

        let prefs = ProxyPreferences {
            exposure: ProxyExposure::Local,
            auth: ProxyAuthMode::Tailnet,
            ..Default::default()
        };

        let result = LocalProxy::start(LocalProxyOptions {
            command,
            preferences: prefs,
            bearer_token: None,
            explicit_env: vec![],
            inherit_env: vec![],
        })
        .await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }
}
