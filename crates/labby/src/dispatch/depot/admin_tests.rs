use super::admin::{
    AdminError, CredentialChange, Mutation, build_remove, build_upsert, change_policy,
};
use crate::config::depot::{AuthMode, ProviderView};

fn provider(endpoint: &str, auth_mode: AuthMode, enabled: bool) -> ProviderView {
    ProviderView {
        id: "team".into(),
        name: "Team".into(),
        endpoint: endpoint.into(),
        enabled,
        auth_mode,
        bearer_token_env: (auth_mode == AuthMode::Bearer)
            .then(|| "LABBY_DEPOT_PROVIDER_TEAM_TOKEN".into()),
        host_managed: false,
        read_project_id: None,
        expected_deployment_id: None,
    }
}

#[test]
fn host_local_ids_and_token_references_cannot_be_mutated_by_browser() {
    let config = r#"
[depot]
read_project_id = "team-project"
[[depot.local_providers]]
id = "team-local"
name = "Host Team"
endpoint = "http://127.0.0.1:4100"
bearer_token_env = "LABBY_DEPOT_PROVIDER_COLLISION_TOKEN"
"#;
    for id in ["team-local", "collision"] {
        let mutation = Mutation {
            id: id.into(),
            name: "Browser override".into(),
            endpoint: "https://example.com".into(),
            enabled: true,
            auth_mode: AuthMode::Bearer,
            credential: CredentialChange::Replace("new-token".into()),
        };
        assert!(matches!(
            build_upsert(config, "", &mutation),
            Err(AdminError::Invalid)
        ));
    }
    assert!(matches!(
        build_remove(config, "", "team-local"),
        Err(AdminError::Invalid)
    ));
    let malformed = config.replace("http://127.0.0.1:4100", "http://not-loopback:4100");
    assert!(matches!(
        build_remove(&malformed, "", "team-local"),
        Err(AdminError::Invalid)
    ));
}

#[test]
fn endpoint_change_can_never_retain_the_old_credential() {
    let old = provider("https://one.example/api", AuthMode::Bearer, true);
    let new = provider("https://one.example/other", AuthMode::Bearer, true);
    assert_eq!(
        change_policy(Some(&old), &new, &CredentialChange::Retain),
        Err(AdminError::FreshAuth)
    );
    assert!(
        change_policy(Some(&old), &new, &CredentialChange::Replace("new".into()))
            .unwrap()
            .needs_fresh_proof
    );
}

#[test]
fn cleared_bearer_must_be_saved_as_a_disabled_draft() {
    assert_eq!(
        change_policy(
            None,
            &provider("https://one.example", AuthMode::Bearer, true),
            &CredentialChange::Clear
        ),
        Err(AdminError::Invalid)
    );
    assert!(
        change_policy(
            Some(&provider("https://one.example", AuthMode::Bearer, true)),
            &provider("https://one.example", AuthMode::Bearer, false),
            &CredentialChange::Clear
        )
        .unwrap()
        .needs_fresh_proof
    );
}

#[test]
fn disable_needs_no_network_qualification() {
    let old = provider("https://one.example", AuthMode::Anonymous, true);
    let policy = change_policy(
        Some(&old),
        &provider("https://one.example", AuthMode::Anonymous, false),
        &CredentialChange::Retain,
    )
    .unwrap();
    assert!(!policy.needs_qualification);
    assert!(!policy.needs_fresh_proof);
}

#[test]
fn operation_metadata_keeps_admin_and_destructive_axes_separate() {
    let upsert = super::operations::ACTIONS
        .iter()
        .find(|action| action.name == "providers.upsert")
        .unwrap();
    let remove = super::operations::ACTIONS
        .iter()
        .find(|action| action.name == "providers.remove")
        .unwrap();
    assert!(upsert.requires_admin && !upsert.destructive);
    assert!(remove.requires_admin && remove.destructive);
}

#[test]
fn upsert_preserves_foreign_toml_and_env_while_rotating_only_owned_secret() {
    let config = "foreign = 'keep'\n[depot]\npublic_enabled = true\n";
    let environment = "# keep me\nOTHER=value\nLABBY_DEPOT_PROVIDER_TEAM_TOKEN=old\n";
    let mutation = Mutation {
        id: "team".into(),
        name: "Team Depot".into(),
        endpoint: "https://team.example/api".into(),
        enabled: true,
        auth_mode: AuthMode::Bearer,
        credential: CredentialChange::Replace("new secret".into()),
    };
    let built = build_upsert(config, environment, &mutation).unwrap();
    assert!(built.pair.config.contains("foreign = 'keep'"));
    assert!(built.pair.config.contains("id = \"team\""));
    assert!(built.pair.environment.contains("# keep me\nOTHER=value\n"));
    assert!(
        built
            .pair
            .environment
            .contains("LABBY_DEPOT_PROVIDER_TEAM_TOKEN=\"new secret\"")
    );
    assert!(!built.pair.environment.contains("=old"));
}

#[test]
fn bearer_credentials_reject_header_and_environment_control_characters() {
    for token in [
        "line\nbreak",
        "carriage\rreturn",
        "tab\tvalue",
        "nul\0value",
    ] {
        let mutation = Mutation {
            id: "team".into(),
            name: "Team Depot".into(),
            endpoint: "https://team.example/api".into(),
            enabled: true,
            auth_mode: AuthMode::Bearer,
            credential: CredentialChange::Replace(token.into()),
        };
        assert_eq!(
            build_upsert("", "", &mutation).unwrap_err(),
            AdminError::Invalid
        );
    }
}

#[test]
fn retained_secret_and_reserved_ids_fail_closed() {
    let mutation = Mutation {
        id: "all".into(),
        name: "Reserved".into(),
        endpoint: "https://team.example".into(),
        enabled: true,
        auth_mode: AuthMode::Bearer,
        credential: CredentialChange::Retain,
    };
    assert_eq!(
        build_upsert("", "", &mutation).unwrap_err(),
        AdminError::Invalid
    );
}

#[test]
fn removal_tombstones_id_and_deletes_only_its_active_secret() {
    let config = "[depot]\n[[depot.providers]]\nid='team'\nname='Team'\nendpoint='https://team.example'\nenabled=true\nauth_mode='bearer'\nbearer_token_env='LABBY_DEPOT_PROVIDER_TEAM_TOKEN'\n";
    let environment = "OTHER=keep\nLABBY_DEPOT_PROVIDER_TEAM_TOKEN=secret\n";
    let built = build_remove(config, environment, "team").unwrap();
    assert!(!built.pair.config.contains("id = \"team\""));
    assert!(built.preferences.tombstones.contains("team"));
    assert_eq!(built.pair.environment, "OTHER=keep\n");
}

#[test]
fn browser_cannot_overwrite_public_binding_credentials_and_toggle_preserves_binding() {
    let config = r#"
[depot]
read_project_id = "catalog-project"
[depot.public_read_binding]
endpoint = "http://127.0.0.1:4101"
bearer_token_env = "LABBY_DEPOT_PROVIDER_COLLISION_TOKEN"
deployment_id = "catalog"
"#;
    let mut mutation = Mutation {
        id: "collision".into(),
        name: "Bad".into(),
        endpoint: "https://example.com".into(),
        enabled: true,
        auth_mode: AuthMode::Bearer,
        credential: CredentialChange::Replace("replacement".into()),
    };
    assert!(matches!(
        build_upsert(config, "", &mutation),
        Err(AdminError::Invalid)
    ));
    mutation.id = "public".into();
    mutation.name = "Public Depot".into();
    mutation.endpoint = crate::config::depot::PUBLIC_ENDPOINT.into();
    mutation.auth_mode = AuthMode::Anonymous;
    mutation.credential = CredentialChange::Retain;
    mutation.enabled = false;
    let result = build_upsert(config, "", &mutation).unwrap();
    assert!(result.preferences.public_read_binding.is_some());
    assert!(result.view.host_managed);
    assert!(!result.view.enabled);
    assert!(
        build_upsert(
            &config.replace("COLLISION_TOKEN", "PUBLIC_TOKEN"),
            "",
            &mutation
        )
        .is_ok()
    );
}
