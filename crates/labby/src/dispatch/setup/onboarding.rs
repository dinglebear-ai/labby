//! First-use provider setup. Configuration, connectivity, and execution are
//! deliberately separate facts: listing models never proves an Agent ran.

use std::{collections::BTreeSet, time::Duration};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::caller::SetupCaller;
use crate::{
    config::env_merge::{self, EnvEntry, MergeRequest},
    dispatch::{
        error::ToolError,
        phoenix_openai::{API_KEY_ENV, BASE_URL_ENV, OpenAiBackend},
    },
    installation::InstallationPaths,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MODELS: usize = 128;
static CONFIGURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct ProviderState {
    configured: bool,
    base_url: Option<String>,
    api_key_configured: bool,
    externally_managed: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct OnboardingState {
    schema_version: u8,
    provider: ProviderState,
    guide_path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct VerifiedProvider {
    provider: ProviderState,
    models: Vec<String>,
    restart_required: bool,
    agent_verified: bool,
}

// Intentionally not Debug or Serialize: this request contains a credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureProvider {
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
}

fn externally_managed() -> bool {
    [BASE_URL_ENV, API_KEY_ENV]
        .iter()
        .any(|key| crate::config::env_key_set_outside_dotenv(key))
}

fn provider_state(backend: Option<&OpenAiBackend>, external: bool) -> ProviderState {
    ProviderState {
        configured: backend.is_some(),
        base_url: backend.map(|value| value.base_url().to_string()),
        api_key_configured: backend.is_some_and(OpenAiBackend::api_key_configured),
        externally_managed: external,
    }
}

pub(super) fn state() -> OnboardingState {
    let backend = OpenAiBackend::from_env();
    OnboardingState {
        schema_version: 1,
        provider: provider_state(backend.as_ref(), externally_managed()),
        guide_path: "/onboarding".into(),
    }
}

pub(super) async fn models() -> Result<VerifiedProvider, ToolError> {
    let backend = OpenAiBackend::from_env()
        .ok_or_else(|| unavailable("Connect an Agent provider before verifying its models."))?;
    verified(&backend, externally_managed()).await
}

pub(super) async fn configure(
    caller: SetupCaller,
    params: &Value,
) -> Result<VerifiedProvider, ToolError> {
    // Surface metadata additionally restricts this new-endpoint probe to the
    // existing trusted-local setup transport. Never promote delegated admins.
    if caller != SetupCaller::Operator {
        return Err(ToolError::Forbidden {
            message: "Only the host operator may connect the first Agent provider.".into(),
            required_scopes: Vec::new(),
        });
    }
    let request: ConfigureProvider = serde_json::from_value(params.clone())
        .map_err(|_| invalid("provider", "Provide a base URL and an optional API key."))?;
    let _guard = CONFIGURE_LOCK.lock().await;
    if OpenAiBackend::from_env().is_some() || externally_managed() {
        return Err(unavailable(
            "An Agent provider is already configured or managed by the service environment. Use Settings to change it; first-run setup never replaces existing credentials.",
        ));
    }
    let paths = InstallationPaths::resolve()
        .map_err(|_| unavailable("Cannot resolve the selected Labby installation."))?;
    let (backend, result) = configure_at(&paths, request).await?;
    // Publish only after durable commit. Existing sessions retain their backend.
    OpenAiBackend::publish_first_run(backend);
    Ok(result)
}

async fn configure_at(
    paths: &InstallationPaths,
    request: ConfigureProvider,
) -> Result<(OpenAiBackend, VerifiedProvider), ToolError> {
    let backend = validate_request(&request)?;
    let env_path = paths.dotenv();
    let expected_mtime = env_merge::snapshot_mtime(&env_path);
    let result = verified(&backend, false).await?;
    let paths = paths.clone();
    let entries = vec![
        EnvEntry::new(BASE_URL_ENV, backend.base_url().as_str()),
        EnvEntry::new(API_KEY_ENV, request.api_key.as_deref().unwrap_or("").trim()),
    ];
    tokio::task::spawn_blocking(move || {
        paths.prepare_root().map_err(|_| unavailable("Cannot prepare protected Labby state."))?;
        // No force, no secret reuse, and an optimistic file snapshot. A late
        // first-run request cannot overwrite another operator configuration.
        env_merge::merge_new_keys(&env_path, MergeRequest { entries, force: false, expected_mtime })
            .map_err(|error| ToolError::Sdk {
                sdk_kind: error.kind().to_string(),
                message: "Provider configuration was not saved. The environment may have changed or contain conflicting values; refresh Settings before retrying.".into(),
            })?;
        Ok::<(), ToolError>(())
    }).await.map_err(|_| unavailable("Provider configuration worker did not complete."))??;
    Ok((backend, result))
}

fn validate_request(request: &ConfigureProvider) -> Result<OpenAiBackend, ToolError> {
    if request.base_url.len() > 2048 {
        return Err(invalid("base_url", "Provider base URL is too long."));
    }
    if request
        .api_key
        .as_ref()
        .is_some_and(|key| key.len() > 8192 || key.chars().any(char::is_control))
    {
        return Err(invalid(
            "api_key",
            "API keys must not contain control characters and must be at most 8192 bytes.",
        ));
    }
    let backend = OpenAiBackend::from_url(request.base_url.trim(), request.api_key.clone())?;
    let url = backend.base_url();
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if backend.api_key_configured() && url.scheme() != "https" && !loopback {
        return Err(invalid(
            "base_url",
            "Use HTTPS for a provider API key. Plain HTTP credentials are allowed only on loopback.",
        ));
    }
    Ok(backend)
}

async fn verified(backend: &OpenAiBackend, external: bool) -> Result<VerifiedProvider, ToolError> {
    let rows = tokio::time::timeout(PROBE_TIMEOUT, backend.models()).await
        .map_err(|_| unavailable("Provider verification timed out after 10 seconds. Check reachability from the Labby gateway host."))?
        // Upstream errors can echo authorization material. Do not return their
        // response bodies or interpolate request credentials into setup logs.
        .map_err(|_| unavailable("Provider verification failed. Check gateway-host reachability, credentials, and support for the /models endpoint."))?;
    let models = model_ids(&rows);
    if models.is_empty() {
        return Err(unavailable(
            "The provider advertised no usable models. No configuration was saved.",
        ));
    }
    Ok(VerifiedProvider {
        provider: provider_state(Some(backend), external),
        models,
        restart_required: false,
        agent_verified: false,
    })
}

fn model_ids(rows: &[Value]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    rows.iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .filter(|id| seen.insert((*id).to_owned()))
        .take(MAX_MODELS)
        .map(str::to_owned)
        .collect()
}

fn invalid(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        param: param.into(),
        message: message.into(),
    }
}

fn unavailable(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unavailable".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn tls() {
        drop(rustls::crypto::ring::default_provider().install_default());
    }

    #[test]
    fn first_use_does_not_equate_configuration_with_execution() {
        let state = provider_state(None, false);
        assert!(!state.configured);
        assert!(!state.api_key_configured);
        let ids = model_ids(&[
            json!({"id":"local-model"}),
            json!({"id":"local-model"}),
            json!({"id":""}),
            json!({"id":"bad\nmodel"}),
            json!({"unexpected":true}),
        ]);
        assert_eq!(ids, vec!["local-model"]);
    }

    #[test]
    fn credentials_require_tls_except_on_loopback() {
        tls();
        for url in [
            "http://127.0.0.1:1234/v1",
            "http://[::1]:1234/v1",
            "https://provider.example/v1",
        ] {
            assert!(
                validate_request(&ConfigureProvider {
                    base_url: url.into(),
                    api_key: Some("test-only-key".into())
                })
                .is_ok()
            );
        }
        for url in [
            "http://provider.example/v1",
            "https://user:secret@provider.example/v1",
            "https://provider.example/v1?key=secret",
        ] {
            assert!(
                validate_request(&ConfigureProvider {
                    base_url: url.into(),
                    api_key: Some("test-only-key".into())
                })
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn delegated_callers_cannot_probe_or_save_credentials() {
        let error = configure(
            SetupCaller::Delegated,
            &json!({"base_url":"http://127.0.0.1:9/v1"}),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ToolError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn verified_provider_is_durable_without_overwriting_other_settings() {
        tls();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"test-model"}]})),
            )
            .mount(&server)
            .await;
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("labby")).unwrap();
        paths.prepare_root().unwrap();
        std::fs::write(paths.dotenv(), "# Preserve this\nUNRELATED=value\n").unwrap();
        let (_, result) = configure_at(
            &paths,
            ConfigureProvider {
                base_url: format!("{}/v1", server.uri()),
                api_key: Some("test-only-key".into()),
            },
        )
        .await
        .unwrap();
        assert!(!result.agent_verified);
        assert!(!result.restart_required);
        assert_eq!(result.models, vec!["test-model"]);
        let stored = std::fs::read_to_string(paths.dotenv()).unwrap();
        assert!(stored.contains("UNRELATED=value"));
        assert!(stored.contains(BASE_URL_ENV));
        assert!(stored.contains("test-only-key"));
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("test-only-key")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(paths.dotenv())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn failed_probe_writes_nothing_and_does_not_echo_provider_secrets() {
        tls();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error":{"message":"echoed-test-only-key"}})),
            )
            .mount(&server)
            .await;
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("labby")).unwrap();
        let result = configure_at(
            &paths,
            ConfigureProvider {
                base_url: format!("{}/v1", server.uri()),
                api_key: None,
            },
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("probe should fail"),
        };
        assert!(!error.to_string().contains("echoed-test-only-key"));
        assert!(!paths.dotenv().exists());
    }
}
