//! Write-only secret stdio environment values on `gateway.add`/`gateway.update`.

use crate::gateway::config::load_gateway_config;
use crate::gateway::config_mutation::read_env_values;
use crate::gateway::params::GatewayUpdatePatch;
use crate::gateway::types::GatewayView;
use crate::security::stdio_secret_env::{MAX_SECRET_ENV_ENTRIES, MAX_SECRET_ENV_VALUE_BYTES};
use labby_runtime::error::ToolError;

use super::*;

const FAKE_A: &str = "fake-secret-a-51f0";
const FAKE_B: &str = "fake-secret-b-77e2";
const FAKE_C: &str = "fake-secret-c-0d93";

struct Fixture {
    dir: tempfile::TempDir,
    manager: GatewayManager,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = GatewayManager::new(dir.path().join("config.toml"), Default::default())
            .with_env_path(dir.path().join(".env"));
        Self { dir, manager }
    }

    fn dotenv(&self) -> HashMap<String, String> {
        read_env_values(&self.dir.path().join(".env")).unwrap_or_default()
    }

    fn config_toml(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("config.toml")).unwrap_or_default()
    }

    fn upstream(&self, name: &str) -> UpstreamConfig {
        load_gateway_config(&self.dir.path().join("config.toml"))
            .expect("config")
            .upstream
            .into_iter()
            .find(|upstream| upstream.name == name)
            .expect("upstream")
    }

    async fn add_figma(&self, values: &[(&str, &str)]) -> Result<GatewayView, ToolError> {
        let mut spec = fixture_stdio_upstream("figma");
        // Configuration is under test; never wait on a real npx handshake.
        spec.enabled = false;
        spec.env
            .insert("FIGMA_MODE".to_string(), "plain".to_string());
        self.manager
            .add_scoped_with_route(
                spec,
                None,
                secret_values(values),
                None,
                None,
                Default::default(),
                None,
            )
            .await
    }

    async fn set(&self, name: &str, values: &[(&str, &str)]) -> Result<GatewayView, ToolError> {
        self.manager
            .update_with_route(
                name,
                GatewayUpdatePatch::default(),
                None,
                secret_values(values),
                None,
                None,
                None,
            )
            .await
    }
}

fn secret_values(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

fn assert_not_exposed(fixture: &Fixture, view: Option<&GatewayView>, error: Option<&ToolError>) {
    let view = view.map(|view| serde_json::to_string(view).expect("view json"));
    let error = error.map(|error| format!("{error} {}", serde_json::to_string(error).unwrap()));
    for fake in [FAKE_A, FAKE_B, FAKE_C] {
        assert!(
            !fixture.config_toml().contains(fake),
            "value in config.toml"
        );
        assert!(
            !view.as_deref().unwrap_or("").contains(fake),
            "value in view"
        );
        assert!(
            !error.as_deref().unwrap_or("").contains(fake),
            "value in error"
        );
    }
}

#[tokio::test]
async fn add_stores_secret_values_in_credential_file_and_names_in_config() {
    let fixture = Fixture::new();

    let view = fixture
        .add_figma(&[("FIGMA_API_KEY", FAKE_A)])
        .await
        .expect("add");

    assert_eq!(view.config.secret_env, vec!["FIGMA_API_KEY".to_string()]);
    let upstream = fixture.upstream("figma");
    assert_eq!(
        upstream.secret_env.get("FIGMA_API_KEY").map(String::as_str),
        Some("LABBY_GW_FIGMA_ENV_FIGMA_API_KEY")
    );
    assert_eq!(
        upstream.env.get("FIGMA_MODE").map(String::as_str),
        Some("plain")
    );
    assert_eq!(
        fixture
            .dotenv()
            .get("LABBY_GW_FIGMA_ENV_FIGMA_API_KEY")
            .map(String::as_str),
        Some(FAKE_A)
    );
    assert_not_exposed(&fixture, Some(&view), None);
}

#[tokio::test]
async fn set_replaces_and_adds_values_without_dropping_other_env() {
    let fixture = Fixture::new();
    fixture
        .add_figma(&[("FIGMA_API_KEY", FAKE_A)])
        .await
        .expect("add");

    let view = fixture
        .set(
            "figma",
            &[("FIGMA_API_KEY", FAKE_B), ("OTHER_TOKEN", FAKE_C)],
        )
        .await
        .expect("set");

    let upstream = fixture.upstream("figma");
    assert_eq!(
        upstream.env.get("FIGMA_MODE").map(String::as_str),
        Some("plain")
    );
    assert_eq!(
        upstream
            .secret_env
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["FIGMA_API_KEY", "OTHER_TOKEN"]
    );
    let dotenv = fixture.dotenv();
    assert_eq!(
        dotenv
            .get("LABBY_GW_FIGMA_ENV_FIGMA_API_KEY")
            .map(String::as_str),
        Some(FAKE_B)
    );
    assert_eq!(
        dotenv
            .get("LABBY_GW_FIGMA_ENV_OTHER_TOKEN")
            .map(String::as_str),
        Some(FAKE_C)
    );
    assert!(
        !dotenv.values().any(|value| value == FAKE_A),
        "old value kept"
    );
    assert_not_exposed(&fixture, Some(&view), None);
}

#[tokio::test]
async fn set_moves_a_plain_env_value_to_the_credential_file() {
    let fixture = Fixture::new();
    fixture.add_figma(&[]).await.expect("add");

    fixture
        .set("figma", &[("FIGMA_MODE", FAKE_A)])
        .await
        .expect("set");

    let upstream = fixture.upstream("figma");
    assert!(!upstream.env.contains_key("FIGMA_MODE"));
    assert!(upstream.secret_env.contains_key("FIGMA_MODE"));
    assert_not_exposed(&fixture, None, None);
}

#[tokio::test]
async fn set_keeps_the_existing_credential_name_after_a_rename() {
    let fixture = Fixture::new();
    fixture
        .add_figma(&[("FIGMA_API_KEY", FAKE_A)])
        .await
        .expect("add");
    fixture
        .manager
        .update(
            "figma",
            GatewayUpdatePatch {
                name: Some("design".to_string()),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .await
        .expect("rename");

    fixture
        .set("design", &[("FIGMA_API_KEY", FAKE_B)])
        .await
        .expect("set");

    assert_eq!(
        fixture.upstream("design").secret_env["FIGMA_API_KEY"],
        "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY"
    );
    assert_eq!(fixture.dotenv()["LABBY_GW_FIGMA_ENV_FIGMA_API_KEY"], FAKE_B);
}

#[tokio::test]
async fn invalid_secret_values_are_refused_before_any_write() {
    let oversized = format!("{FAKE_A}{}", "x".repeat(MAX_SECRET_ENV_VALUE_BYTES));
    let too_many = (0..=MAX_SECRET_ENV_ENTRIES)
        .map(|index| (format!("KEY_{index}"), FAKE_A.to_string()))
        .collect::<Vec<_>>();
    let too_many = too_many
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("empty value", vec![("FIGMA_API_KEY", "")]),
        ("blank value", vec![("FIGMA_API_KEY", "   ")]),
        (
            "oversized value",
            vec![("FIGMA_API_KEY", oversized.as_str())],
        ),
        (
            "line break",
            vec![("FIGMA_API_KEY", "fake-secret-a-51f0\nX=1")],
        ),
        ("lowercase name", vec![("figma_api_key", FAKE_A)]),
        ("reserved name", vec![("PATH", FAKE_A)]),
        ("sandbox name", vec![("UPSTREAM_STATE_DIR", FAKE_A)]),
        ("labby name", vec![("LABBY_HOME", FAKE_A)]),
        ("loader name", vec![("LD_PRELOAD", FAKE_A)]),
        ("too many", too_many),
    ];
    for (case, values) in cases {
        let fixture = Fixture::new();
        let error = fixture.add_figma(&values).await.expect_err(case);
        assert_eq!(error.kind(), "invalid_param", "{case}: {error}");
        assert!(fixture.dotenv().is_empty(), "{case}: credential written");
        assert!(
            !fixture.config_toml().contains("figma"),
            "{case}: config written"
        );
        assert_not_exposed(&fixture, None, Some(&error));
    }
}

#[tokio::test]
async fn secret_values_require_a_stdio_upstream() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .add_scoped_with_route(
            fixture_http_upstream("remote"),
            None,
            secret_values(&[("FIGMA_API_KEY", FAKE_A)]),
            None,
            None,
            Default::default(),
            None,
        )
        .await
        .expect_err("http upstream");

    assert_eq!(error.kind(), "invalid_param");
    assert!(fixture.dotenv().is_empty());
    assert_not_exposed(&fixture, None, Some(&error));
}

#[tokio::test]
async fn secret_name_must_not_shadow_the_bearer_token_variable() {
    let fixture = Fixture::new();
    let mut spec = fixture_stdio_upstream("figma");
    spec.bearer_token_env = Some("FIGMA_API_KEY".to_string());

    let error = fixture
        .manager
        .add_scoped_with_route(
            spec,
            None,
            secret_values(&[("FIGMA_API_KEY", FAKE_A)]),
            None,
            None,
            Default::default(),
            None,
        )
        .await
        .expect_err("collision");

    assert_eq!(error.kind(), "invalid_param");
    assert!(fixture.dotenv().is_empty());
}

#[tokio::test]
async fn spec_cannot_point_a_child_at_an_unrelated_installation_secret() {
    let fixture = Fixture::new();
    for credential in ["LABBY_OAUTH_ENCRYPTION_KEY", "LABBY_GW_OTHER_AUTH_HEADER"] {
        let mut spec = fixture_stdio_upstream("figma");
        spec.enabled = false;
        spec.secret_env
            .insert("FIGMA_API_KEY".to_string(), credential.to_string());

        let error = fixture
            .manager
            .add(spec, None, None, None)
            .await
            .expect_err(credential);

        assert_eq!(error.kind(), "invalid_param", "{credential}");
        assert!(!fixture.config_toml().contains("figma"));
    }
}
