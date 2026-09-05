use super::discovery::{
    DiscoveryError, ProviderPage, merge_page, project_detail, provider_request_limit,
    validate_request,
};
use serde_json::json;

fn page(id: &str, count: usize) -> ProviderPage {
    ProviderPage::participating(
        id,
        (0..count)
            .map(|n| json!({"id": format!("{id}-{n}"), "name": format!("row {n}")}))
            .collect(),
        None,
        Some(count as u64),
    )
}

#[test]
fn provider_request_never_exceeds_the_advertised_page_size() {
    assert_eq!(provider_request_limit(150, Some(25)), 25);
    assert_eq!(provider_request_limit(20, Some(25)), 20);
}

#[test]
fn exact_detail_rejects_identity_substitution_and_drops_untrusted_metadata() {
    let raw = json!({
        "id": "artifact-1", "descriptor": {"id":"artifact-1", "name":"safe"},
        "currentRevisionId":"rev-1", "metadata":{"callbackUrl":"http://attacker"}
    });
    let projected = project_detail("artifact-1", raw.clone()).unwrap();
    assert!(projected.get("metadata").is_none());
    assert_eq!(
        project_detail("artifact-2", raw),
        Err(DiscoveryError::InvalidProvider)
    );
}

#[test]
fn deterministic_round_robin_is_fair_and_provider_qualifies_identity() {
    let mut providers = vec![page("alpha", 20), page("beta", 2), page("gamma", 2)];
    let response = merge_page(&mut providers, 0, 6).unwrap();
    let ids: Vec<_> = response
        .items
        .iter()
        .map(|row| row["artifactId"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "alpha-0", "beta-0", "gamma-0", "alpha-1", "beta-1", "gamma-1"
        ]
    );
    assert!(
        response
            .items
            .iter()
            .all(|row| row["providerId"].is_string())
    );
    assert_eq!(response.known_total, Some(24));
    assert!(response.total_is_exact);
}

#[test]
fn pending_is_deferred_while_actual_failure_is_partial() {
    let mut pending = vec![ProviderPage::pending("alpha")];
    assert_eq!(merge_page(&mut pending, 0, 50).unwrap().state, "deferred");
    let mut partial = vec![
        page("alpha", 1),
        ProviderPage::failed("beta", "unavailable"),
    ];
    let response = merge_page(&mut partial, 0, 50).unwrap();
    assert_eq!(response.state, "partial");
    assert!(!response.coverage_complete);
}

#[test]
fn request_and_projection_bounds_fail_closed() {
    assert_eq!(
        validate_request("ab", 50),
        Err(DiscoveryError::InvalidQuery)
    );
    assert_eq!(
        validate_request("valid", 201),
        Err(DiscoveryError::InvalidLimit)
    );
    let mut bad = vec![ProviderPage::participating(
        "alpha",
        vec![json!({"name":"missing id"})],
        None,
        None,
    )];
    assert_eq!(
        merge_page(&mut bad, 0, 50).unwrap_err(),
        DiscoveryError::InvalidProvider
    );
}
