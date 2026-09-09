use super::discovery::{
    DiscoveryError, ProviderPage, merge_page, project_detail, provider_list_body,
    provider_request_limit, validate_request,
};
use serde_json::json;

#[test]
fn unfiltered_provider_list_omits_absent_fields_but_preserves_search_and_cursor() {
    for query in ["", " ", "\t\n", "   "] {
        validate_request(query, 25).unwrap();
        assert_eq!(
            provider_list_body(query, None, 25, None),
            json!({"limit":25})
        );
    }
    assert_eq!(
        provider_list_body("  artifact search  ", None, 10, Some("next")),
        json!({"query":"  artifact search  ","limit":10,"cursor":"next"})
    );
}

#[test]
fn real_depot_null_display_fields_and_extra_license_metadata_are_projected() {
    let source = json!({
        "id":"artifact-1", "name":"Team skill", "title":null,"description":null,
        "currentRevisionId":"revision-1", "contentDigest":"sha256:source",
        "license":{"redistribution":"unknown","reviewState":"unreviewed","takedownState":"clear","declared":null,"schemaVersion":1},
        "publication":{"visibility":"private","state":"listed","distribution":"metadata","metadata":{},"schemaVersion":1}
    });
    let mut pages = [ProviderPage::participating(
        "team",
        vec![source],
        None,
        Some(1),
    )];
    let result = merge_page(&mut pages, 0, 10).unwrap();
    let item = &result.items[0];
    assert!(item.get("title").is_none());
    assert!(item.get("description").is_none());
    assert_eq!(
        item["license"],
        json!({"redistribution":"unknown","reviewState":"unreviewed","takedownState":"clear","declared":null})
    );
    assert_eq!(
        item["publication"],
        json!({"visibility":"private","state":"listed","distribution":"metadata"})
    );
    for declared in [json!(null), json!("MIT")] {
        let detail = project_detail("artifact-1", json!({
            "descriptor":{"id":"artifact-1","name":"Team skill","title":null,"description":"Description","schemaVersion":1,"metadata":{},"tags":[]},
            "currentRevisionId":"revision-1",
            "currentRevision":{"id":"revision-1","contentDigest":"sha256:source","components":[],"authoredAt":null,"schemaVersion":1},
            "license":{"redistribution":"unknown","reviewState":"unreviewed","takedownState":"clear","declared":declared,"detected":[],"metadata":{}},
            "publication":{"visibility":"private","state":"listed","distribution":"metadata","publishedAt":null,"schemaVersion":1}
        })).unwrap();
        assert_eq!(
            detail["descriptor"],
            json!({"id":"artifact-1","name":"Team skill","description":"Description"})
        );
        assert_eq!(
            detail["currentRevision"],
            json!({"id":"revision-1","contentDigest":"sha256:source","authoredAt":null})
        );
        assert_eq!(detail["license"]["declared"], declared);
        assert!(detail["license"].get("detected").is_none());
        assert!(detail["license"].get("metadata").is_none());
        assert!(detail["publication"].get("publishedAt").is_none());
    }
}

#[test]
fn projection_rejects_wrong_known_types_and_identity_conflicts() {
    for bad in [
        json!({"id":"artifact-1","title":42}),
        json!({"id":"artifact-1","name":null}),
        json!({"id":"artifact-1","license":{"redistribution":false}}),
        json!({"id":"artifact-1","license":{"declared": {"text":"MIT"}}}),
        json!({"id":"artifact-1","license":{"declared": "x".repeat(1025)}}),
        json!({"id":"artifact-1","publication":{"visibility":[]}}),
        json!({"id":"artifact-1","currentRevision":{"id":null}}),
        json!({"id":"artifact-1","revisionCount":-1}),
    ] {
        let mut pages = [ProviderPage::participating(
            "team",
            vec![bad],
            None,
            Some(1),
        )];
        assert_eq!(
            merge_page(&mut pages, 0, 10).unwrap_err(),
            DiscoveryError::InvalidProvider
        );
    }
    assert_eq!(
        project_detail(
            "artifact-1",
            json!({"id":"artifact-1","descriptor":{"id":"artifact-other"}})
        ),
        Err(DiscoveryError::InvalidProvider)
    );
    assert_eq!(
        project_detail(
            "artifact-1",
            json!({"descriptor":{"id":"artifact-1","title":false}})
        ),
        Err(DiscoveryError::InvalidProvider)
    );
}

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
fn list_and_detail_preserve_bounded_real_timestamps() {
    let raw = json!({
        "id": "dated", "descriptor": {"id": "dated"},
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": null,
        "currentRevision": {
            "id": "rev-1", "contentDigest": "sha256:exact",
            "authoredAt": "2026-09-03T00:00:00Z", "metadata": {"private": "omit"}
        }
    });
    let detail = project_detail("dated", raw.clone()).unwrap();
    let mut pages = vec![ProviderPage::participating(
        "alpha",
        vec![raw],
        None,
        Some(1),
    )];
    let response = merge_page(&mut pages, 0, 1).unwrap();
    for artifact in [&detail, &response.items[0]] {
        assert_eq!(artifact["createdAt"], "2026-09-01T00:00:00Z");
        assert!(artifact["updatedAt"].is_null());
        assert_eq!(
            artifact["currentRevision"]["authoredAt"],
            "2026-09-03T00:00:00Z"
        );
    }
    assert!(
        response.items[0]["currentRevision"]
            .get("metadata")
            .is_none()
    );
}

#[test]
fn list_and_detail_reject_oversized_or_structured_timestamps() {
    for field in ["createdAt", "updatedAt", "authoredAt"] {
        for value in [
            json!("x".repeat(129)),
            json!({"url": "http://attacker"}),
            json!(42),
            json!("javascript:alert(1)"),
            json!("2026-99-99T00:00:00Z"),
        ] {
            let mut raw = json!({"id": "dated", "descriptor": {"id": "dated"}});
            if field == "authoredAt" {
                raw["currentRevision"] = json!({"authoredAt": value});
            } else {
                raw[field] = value;
            }
            assert_eq!(
                project_detail("dated", raw.clone()),
                Err(DiscoveryError::InvalidProvider)
            );
            let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
            assert_eq!(
                merge_page(&mut pages, 0, 1).unwrap_err(),
                DiscoveryError::InvalidProvider
            );
        }
    }
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
