use super::admin::{AdminError, CredentialChange, change_policy};
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
    }
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
