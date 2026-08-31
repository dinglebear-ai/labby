use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::action_matrix::{CaseIntent, EvidenceLevel, ScenarioKind, ScenarioOwner, Surface};
use crate::live_labby::isolated_command;

pub(crate) const MATRIX_DEADLINE: Duration = Duration::from_secs(90);
pub(crate) const CHILD_DEADLINE: Duration = Duration::from_secs(12);
pub(crate) const MAX_CHILDREN: usize = 4;
pub(crate) const RESPONSE_LIMIT: usize = 1024 * 1024;
pub(crate) const SECRET_CANARY: &str = "live-action-matrix-secret-canary";
const _: () = assert!(MAX_CHILDREN > 0 && MAX_CHILDREN <= 4);
const ACTION_CATALOG: &str = include_str!("../../../../docs/generated/action-catalog.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceFixture {
    pub(crate) service: String,
    pub(crate) api_path: Option<String>,
    pub(crate) cli_probe: Option<Vec<String>>,
    pub(crate) can_mutate: bool,
    pub(crate) success_action: String,
    pub(crate) invalid_action: String,
    pub(crate) policy_action: Option<String>,
    pub(crate) workflow: Vec<String>,
    pub(crate) parameters: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) action_params: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Disposition {
    MetadataProbe,
    LiveDispatch,
    IsolatedWorkflow,
    AuthorizationDenial,
    ConditionalProbe,
    OfflineError,
    ReviewedExclusion,
}

#[derive(Clone, Debug)]
pub(crate) struct ActionOutcome {
    pub(crate) key: String,
    pub(crate) surface: Surface,
    pub(crate) disposition: Disposition,
    pub(crate) evidence: EvidenceLevel,
    pub(crate) owner: ScenarioOwner,
    pub(crate) outcome_kind: String,
    pub(crate) recovery: String,
    pub(crate) side_effects: String,
    pub(crate) canary_free: bool,
}

impl ActionOutcome {
    pub(crate) fn satisfies(&self, intent: &CaseIntent) -> bool {
        self.key == intent.key()
            && self.evidence >= intent.minimum_evidence
            && self.owner == intent.scenario_owner
            && !self.outcome_kind.is_empty()
            && !self.recovery.is_empty()
            && !self.side_effects.is_empty()
            && self.canary_free
    }

    pub(crate) fn record(&self) {
        let Some(directory) = std::env::var_os("LABBY_E2E_CASE_DIR") else {
            return;
        };
        let run_id = std::env::var("LABBY_E2E_RUN_ID").expect("run id for case evidence");
        let seed = std::env::var("LABBY_E2E_SEED").expect("seed for case evidence");
        let build_identity =
            std::env::var("LABBY_E2E_BUILD_IDENTITY").expect("build identity for case evidence");
        let event = json!({
            "schema_version": 1,
            "run_id": run_id,
            "seed": seed,
            "build_identity": build_identity,
            "case_id": format!("action::{:?}::{}", self.surface, self.key),
            "kind": "action",
            "achieved_evidence": format!("{:?}", self.evidence),
            "handler_success": matches!(self.evidence, EvidenceLevel::LiveSuccess | EvidenceLevel::LiveStateTransition),
            "denial_only": self.evidence == EvidenceLevel::LiveErrorPath
                && self.outcome_kind.to_ascii_lowercase().contains("den"),
            "outcome_kind": self.outcome_kind,
            "cleanup_ok": self.canary_free,
        });
        write_case_event(&directory, &event);
    }
}

fn write_case_event(directory: &std::ffi::OsStr, event: &Value) {
    use sha2::Digest as _;
    let directory = Path::new(directory);
    std::fs::create_dir_all(directory).expect("create case evidence directory");
    let id = event["case_id"].as_str().expect("case id");
    let name = hex::encode(sha2::Sha256::digest(id.as_bytes()));
    let target = directory.join(format!("{name}.json"));
    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(event).expect("serialize case event"),
    )
    .expect("write case evidence");
    std::fs::rename(temporary, target).expect("publish case evidence");
}

pub(crate) fn disposition(intent: &CaseIntent) -> Disposition {
    match intent.scenario_kind {
        ScenarioKind::ContractProbe => Disposition::MetadataProbe,
        ScenarioKind::LiveInvoke => Disposition::LiveDispatch,
        ScenarioKind::StatefulScenario => Disposition::IsolatedWorkflow,
        ScenarioKind::DestructiveIsolated => Disposition::AuthorizationDenial,
        ScenarioKind::ConditionalOptional => Disposition::ConditionalProbe,
        ScenarioKind::ExternalOptional => Disposition::OfflineError,
        ScenarioKind::ExcludedWithReason => Disposition::ReviewedExclusion,
    }
}

pub(crate) fn fixtures() -> BTreeMap<String, ServiceFixture> {
    let values = [
        include_str!("../fixtures/e2e_actions/doctor.json"),
        include_str!("../fixtures/e2e_actions/fs.json"),
        include_str!("../fixtures/e2e_actions/gateway.json"),
        include_str!("../fixtures/e2e_actions/lab_admin.json"),
        include_str!("../fixtures/e2e_actions/server_logs.json"),
        include_str!("../fixtures/e2e_actions/setup.json"),
        include_str!("../fixtures/e2e_actions/snippets.json"),
        include_str!("../fixtures/e2e_actions/skills.json"),
    ];
    let fixtures = values
        .into_iter()
        .map(|raw| {
            let fixture: ServiceFixture = serde_json::from_str(raw).expect("valid action fixture");
            (fixture.service.clone(), fixture)
        })
        .collect::<BTreeMap<_, _>>();
    let expected_services = crate::action_matrix::intents()
        .iter()
        .map(|intent| intent.service.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fixtures.keys().cloned().collect::<BTreeSet<_>>(),
        expected_services
    );
    let catalog: Vec<crate::action_matrix::CatalogAction> =
        serde_json::from_str(ACTION_CATALOG).expect("action catalog");
    let catalog_by_action = catalog
        .iter()
        .map(|entry| ((entry.service.as_str(), entry.action.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    for (service, fixture) in &fixtures {
        let intents = crate::action_matrix::intents()
            .iter()
            .filter(|intent| &intent.service == service)
            .collect::<Vec<_>>();
        let required = intents
            .iter()
            .flat_map(|intent| intent.fixture_params.parameters.values())
            .map(|source| {
                source
                    .strip_prefix("$fixture.")
                    .unwrap_or_else(|| panic!("non-declarative fixture source {source}"))
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            fixture.parameters.keys().cloned().collect::<BTreeSet<_>>(),
            required
        );
        let action_keys = intents
            .iter()
            .map(|intent| intent.action.clone())
            .collect::<BTreeSet<_>>();
        assert!(
            fixture
                .action_params
                .keys()
                .all(|action| action_keys.contains(action))
        );
        for (action, values) in &fixture.action_params {
            let metadata = catalog_by_action
                .get(&(service.as_str(), action.as_str()))
                .expect("fixture override action metadata");
            let metadata_keys = metadata
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<BTreeSet<_>>();
            assert!(
                values
                    .keys()
                    .all(|name| metadata_keys.contains(name.as_str()))
            );
        }
    }
    fixtures
}

pub(crate) fn exact_plans(surface: Surface) -> BTreeMap<String, Disposition> {
    crate::action_matrix::intents()
        .iter()
        .filter(|intent| intent.applicable_surfaces.contains(&surface))
        .map(|intent| (intent.key(), disposition(intent)))
        .collect()
}

pub(crate) async fn run_cli_probe(home: &Path, args: &[String]) -> Result<Output, String> {
    let mut command = tokio::process::Command::from(isolated_command(home));
    command.args(args).env("LABBY_MATRIX_CANARY", SECRET_CANARY);
    tokio::time::timeout(CHILD_DEADLINE, command.output())
        .await
        .map_err(|_| format!("CLI child exceeded {CHILD_DEADLINE:?}"))?
        .map_err(|error| error.to_string())
}

pub(crate) async fn run_cli(home: &Path, args: &[&str]) -> Result<Output, String> {
    let owned = args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    run_cli_probe(home, &owned).await
}

pub(crate) async fn run_cli_in_install(
    home: &Path,
    labby_home: &Path,
    args: &[&str],
) -> Result<Output, String> {
    let mut command = tokio::process::Command::from(isolated_command(home));
    command
        .env("LABBY_HOME", labby_home)
        .env("LABBY_MATRIX_CANARY", SECRET_CANARY)
        .args(args);
    tokio::time::timeout(CHILD_DEADLINE, command.output())
        .await
        .map_err(|_| format!("CLI child exceeded {CHILD_DEADLINE:?}"))?
        .map_err(|error| error.to_string())
}

pub(crate) fn assert_sanitized(bytes: &[u8], context: &str) {
    assert!(
        bytes.len() <= RESPONSE_LIMIT,
        "{context} exceeded response bound"
    );
    let text = String::from_utf8_lossy(bytes);
    assert!(
        !text.contains(SECRET_CANARY),
        "{context} leaked secret canary"
    );
}

pub(crate) fn assert_json_or_help(output: &Output, context: &str) {
    assert_sanitized(&output.stdout, context);
    assert_sanitized(&output.stderr, context);
    let json = serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success() || json.is_ok(),
        "{context} failed without a stable JSON result: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        json.is_ok() || stdout.contains("Usage:") || stdout.starts_with("Lab ·"),
        "{context} was neither stable JSON nor clap help: {stdout}"
    );
}

pub(crate) fn action_request(intent: &CaseIntent) -> Value {
    json!({"action": intent.action, "params": fixture_params(intent)})
}

pub(crate) fn fixture_params(intent: &CaseIntent) -> Value {
    let all = fixtures();
    let fixture = all.get(&intent.service).expect("service fixture");
    let mut params = serde_json::Map::new();
    for (name, source) in &intent.fixture_params.parameters {
        let fixture_key = source
            .strip_prefix("$fixture.")
            .unwrap_or_else(|| panic!("non-declarative source for {}", intent.key()));
        params.insert(name.clone(), fixture.parameters[fixture_key].clone());
    }
    if let Some(overrides) = fixture.action_params.get(&intent.action) {
        for (name, value) in overrides {
            params.insert(name.clone(), value.clone());
        }
    }
    Value::Object(params)
}

pub(crate) fn services_for(surface: Surface) -> BTreeSet<String> {
    crate::action_matrix::intents()
        .iter()
        .filter(|intent| intent.applicable_surfaces.contains(&surface))
        .map(|intent| intent.service.clone())
        .collect()
}

pub(crate) fn dedicated_contract_reason(key: &str) -> Option<&'static str> {
    match key {
        "gateway:gateway.clients.list" => Some("catalog_dispatch_mismatch"),
        "gateway:gateway.enrich.apply" | "gateway:gateway.enrich.preview" => {
            Some("requires_live_catalog_suggestion")
        }
        key if key.starts_with("gateway:gateway.import") => {
            Some("requires_external_client_import_artifact")
        }
        key if key.starts_with("gateway:gateway.loadout.stage_") => {
            Some("requires_mounted_publication_restart_generation")
        }
        "gateway:gateway.oauth.google_revoke" => Some("requires_stored_google_grant"),
        key if key.starts_with("gateway:gateway.protected_route.stage_") => {
            Some("requires_mounted_publication_restart_generation")
        }
        key if key.starts_with("gateway:gateway.service_config.") => {
            Some("requires_configured_external_builtin_api")
        }
        key if key.starts_with("gateway:gateway.virtual_server.") => {
            Some("requires_migration_created_virtual_server")
        }
        "setup:plugin.install" | "setup:plugin.uninstall" => {
            Some("requires_configured_external_plugin_service")
        }
        "setup:settings.config.update" | "setup:settings.env.update" => {
            Some("typed_compare_and_swap_contract_probed")
        }
        "skills:skills.get" | "skills:skills.read" => Some("requires_indexed_packaged_skill"),
        "snippets:snippets.promote" => Some("requires_real_code_mode_execution_record"),
        _ => None,
    }
}
