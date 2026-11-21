//! Restricted launch policy shared by the managed stdio upstream selectors.
use std::path::PathBuf;

use labby_runtime::{error::ToolError, gateway_config::UpstreamConfig};

use super::{guided_cli, guided_openapi};

#[derive(Clone, Copy)]
pub(crate) enum GuidedStdio {
    Cli,
    Openapi,
}

impl GuidedStdio {
    pub(crate) fn from_command(command: &str) -> Option<Self> {
        match command {
            guided_cli::SELECTOR => Some(Self::Cli),
            guided_openapi::SELECTOR => Some(Self::Openapi),
            _ => None,
        }
    }

    /// Run the same narrow policy at admission and again before connecting.
    pub(crate) fn prepare(
        self,
        config: &UpstreamConfig,
        args: &[String],
    ) -> Result<PathBuf, ToolError> {
        if !config.env.is_empty()
            || !config.secret_env.is_empty()
            || config.bearer_token_env.is_some()
            || config.oauth.is_some()
            || !config.headers.is_empty()
            || !config.headers_env.is_empty()
            || config.lifecycle.is_some()
        {
            return Err(ToolError::InvalidParam {
                message: "guided upstreams do not accept environment, credentials, or lifecycle overrides".into(),
                param: "command".into(),
            });
        }
        match self {
            Self::Cli => guided_cli::prepare(args),
            Self::Openapi => guided_openapi::prepare(args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_selectors_reject_runtime_overrides_before_resolving_host_paths() {
        for selector in [guided_cli::SELECTOR, guided_openapi::SELECTOR] {
            for overrides in [
                serde_json::json!({"env": {"FIXTURE_OPTION": "fixture"}}),
                serde_json::json!({"secret_env": {"FIXTURE_OPTION": "LABBY_FIXTURE_CREDENTIAL"}}),
                serde_json::json!({"bearer_token_env": "FIXTURE_TOKEN"}),
                serde_json::json!({"headers_env": {"x-fixture": "FIXTURE_TOKEN"}}),
                serde_json::json!({"lifecycle": "auto"}),
            ] {
                let mut spec = serde_json::json!({"name": "fixture", "command": selector});
                spec.as_object_mut()
                    .unwrap()
                    .extend(overrides.as_object().unwrap().clone());
                let config: UpstreamConfig = serde_json::from_value(spec).unwrap();
                assert!(
                    GuidedStdio::from_command(selector)
                        .unwrap()
                        .prepare(&config, &config.args)
                        .unwrap_err()
                        .to_string()
                        .contains("overrides")
                );
            }
        }
        // Installing either guided adapter leaves the ordinary executable
        // names on the existing generic policy, not on the managed path.
        assert!(GuidedStdio::from_command("labby").is_none());
        assert!(GuidedStdio::from_command("labby-openapi-mcp").is_none());
        assert!(GuidedStdio::from_command("labby-openapi-mcp.exe").is_none());
    }
}
