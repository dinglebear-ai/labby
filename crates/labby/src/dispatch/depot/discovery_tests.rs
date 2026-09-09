use super::discovery::{
    DiscoveryError, ProviderPage, merge_page, project_detail, provider_list_body,
    provider_request_limit, validate_request,
};
use serde_json::{Value, json};

#[test]
fn lineage_is_strict_nullable_reference_metadata_for_details_only() {
    let base = json!({"id":"artifact","descriptor":{"id":"artifact"}});
    let lineage = json!({"upstreamArtifactId":"upstream","upstreamRevisionId":"revision",
        "forkedFromArtifactId":"source","forkedFromRevisionId":null,"following":true,
        "lastObservedUpstreamRevisionId":format!("sha256:{}","a".repeat(64))});
    assert!(
        project_detail("artifact", base.clone())
            .unwrap()
            .get("lineage")
            .is_none()
    );
    let mut raw = base.clone();
    raw["lineage"] = lineage.clone();
    assert_eq!(
        project_detail("artifact", raw.clone()).unwrap()["lineage"],
        lineage
    );
    let mut pages = vec![ProviderPage::participating(
        "provider",
        vec![raw],
        None,
        None,
    )];
    assert!(
        merge_page(&mut pages, 0, 1).unwrap().items[0]
            .get("lineage")
            .is_none()
    );
    for id in ["a".repeat(160), "0_valid-id".into()] {
        let mut raw = base.clone();
        raw["lineage"] = lineage.clone();
        raw["lineage"]["upstreamArtifactId"] = json!(id);
        assert!(project_detail("artifact", raw).is_ok());
    }
    let mut nullable = lineage.clone();
    for field in [
        "upstreamArtifactId",
        "upstreamRevisionId",
        "forkedFromArtifactId",
        "forkedFromRevisionId",
        "lastObservedUpstreamRevisionId",
    ] {
        nullable[field] = json!(null);
    }
    let mut raw = base.clone();
    raw["lineage"] = nullable;
    assert!(project_detail("artifact", raw).is_ok());
    for field in [
        "upstreamArtifactId",
        "upstreamRevisionId",
        "forkedFromArtifactId",
        "forkedFromRevisionId",
        "following",
        "lastObservedUpstreamRevisionId",
    ] {
        let mut raw = base.clone();
        raw["lineage"] = lineage.clone();
        raw["lineage"].as_object_mut().unwrap().remove(field);
        assert!(project_detail("artifact", raw).is_err(), "missing {field}");
    }
    for (field, value) in [
        ("upstreamArtifactId", json!("")),
        ("upstreamArtifactId", json!("a".repeat(161))),
        ("upstreamArtifactId", json!("Upper")),
        ("upstreamArtifactId", json!("é")),
        ("upstreamArtifactId", json!("_start")),
        ("upstreamArtifactId", json!(null)),
        ("upstreamRevisionId", json!("sha256:ABC")),
        (
            "upstreamRevisionId",
            json!(format!("sha256:{}", "A".repeat(64))),
        ),
        ("upstreamRevisionId", json!(false)),
        ("following", json!(null)),
        ("following", json!("true")),
        ("source", json!("private")),
    ] {
        let mut raw = base.clone();
        raw["lineage"] = lineage.clone();
        raw["lineage"][field] = value;
        assert!(project_detail("artifact", raw).is_err(), "invalid {field}");
    }
    let mut raw = base.clone();
    raw["lineage"] = lineage.clone();
    raw["lineage"]["upstreamArtifactId"] = json!(null);
    raw["lineage"]["upstreamRevisionId"] = json!(null);
    assert!(
        project_detail("artifact", raw).is_err(),
        "observed revision requires upstream"
    );
    let mut raw = base;
    raw["lineage"] = lineage;
    raw["lineage"]["forkedFromArtifactId"] = json!(null);
    raw["lineage"]["forkedFromRevisionId"] = json!("revision");
    assert!(
        project_detail("artifact", raw).is_err(),
        "fork revision requires source"
    );
}

#[test]
fn readme_detail_is_exact_revision_bound_and_never_leaks_into_listing() {
    let base = json!({"id":"artifact", "descriptor":{"id":"artifact"},
        "currentRevision":{"id":"revision"},"currentRevisionId":"revision"});
    assert!(
        project_detail("artifact", base.clone())
            .unwrap()
            .get("readme")
            .is_none()
    );
    for (kind, path) in [("readme", "README.md"), ("skill", "SKILL.md")] {
        for content in [String::new(), "é".repeat(32768)] {
            let mut raw = base.clone();
            raw["readme"] = json!({"state":"available","kind":kind,"path":path,
                "content":content,"revisionId":"revision"});
            assert_eq!(
                project_detail("artifact", raw.clone()).unwrap()["readme"],
                raw["readme"]
            );
            let mut pages = vec![ProviderPage::participating(
                "provider",
                vec![raw],
                None,
                None,
            )];
            assert!(
                merge_page(&mut pages, 0, 1).unwrap().items[0]
                    .get("readme")
                    .is_none()
            );
        }
    }
    for reason in [
        "absent",
        "not_distributable",
        "too_large",
        "storage_unavailable",
        "invalid_text",
    ] {
        let mut raw = base.clone();
        raw["readme"] = json!({"state":"unavailable","reason":reason});
        assert_eq!(
            project_detail("artifact", raw.clone()).unwrap()["readme"],
            raw["readme"]
        );
    }
    let available = json!({"state":"available","kind":"readme","path":"README.md","content":"hello","revisionId":"revision"});
    for (field, value) in [
        ("kind", json!("skill")),
        ("path", json!("../README.md")),
        ("revisionId", json!("other")),
        ("revisionId", json!("")),
        ("revisionId", json!("r".repeat(513))),
        ("content", json!("é".repeat(32769))),
        ("content", json!("hello\0world")),
        ("content", json!(null)),
        ("state", json!("unknown")),
        ("source", json!("private-source")),
    ] {
        let mut raw = base.clone();
        let mut readme = available.clone();
        readme[field] = value;
        raw["readme"] = readme;
        assert!(
            project_detail("artifact", raw).is_err(),
            "accepted invalid {field}"
        );
    }
    for field in ["state", "kind", "path", "content", "revisionId"] {
        let mut raw = base.clone();
        let mut readme = available.clone();
        readme.as_object_mut().unwrap().remove(field);
        raw["readme"] = readme;
        assert!(project_detail("artifact", raw).is_err());
    }
    for readme in [
        json!(null),
        json!([]),
        json!({"state":"unavailable","reason":"unknown"}),
        json!({"state":"unavailable","reason":"absent","content":"secret"}),
    ] {
        let mut raw = base.clone();
        raw["readme"] = readme;
        assert!(project_detail("artifact", raw).is_err());
    }
    for field in ["currentRevision", "currentRevisionId"] {
        let mut raw = base.clone();
        raw["readme"] = available.clone();
        raw[field] = json!("other");
        assert!(project_detail("artifact", raw).is_err());
    }
    let mut raw = base;
    raw["readme"] = available;
    raw.as_object_mut().unwrap().remove("currentRevision");
    assert!(project_detail("artifact", raw).is_err());
}

#[test]
fn unfiltered_provider_list_omits_absent_fields_but_preserves_search_and_cursor() {
    for query in ["", " ", "\t\n", "   "] {
        validate_request(query, 25).unwrap();
        assert_eq!(provider_list_body(query, 25, None), json!({"limit":25}));
    }
    assert_eq!(
        provider_list_body("  artifact search  ", 10, Some("next")),
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
            json!({"id":"artifact-1","name":"Team skill","description":"Description","tags":[]})
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
fn list_and_detail_preserve_bounded_file_counts() {
    for count in [None, Some(0), Some(1), Some(2000)] {
        let mut raw = json!({"id": "files", "descriptor": {"id": "files"}, "currentRevision": {"id": "rev", "components": [], "metadata": {"private": true}}});
        if let Some(count) = count {
            raw["currentRevision"]["fileCount"] = json!(count);
        }
        let detail = project_detail("files", raw.clone()).unwrap();
        let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
        let response = merge_page(&mut pages, 0, 1).unwrap();
        for artifact in [&detail, &response.items[0]] {
            assert_eq!(
                artifact["currentRevision"].get("fileCount"),
                count.map(|value| json!(value)).as_ref()
            );
            assert!(artifact["currentRevision"].get("components").is_none());
            assert!(artifact["currentRevision"].get("metadata").is_none());
        }
    }
}

#[test]
fn detail_preserves_exact_optional_descriptor_tags() {
    for tags in [
        None,
        Some(json!([])),
        Some(json!(["Review", "with spaces", "é".repeat(32)])),
        Some(json!((0..64).map(|i| i.to_string()).collect::<Vec<_>>())),
    ] {
        let mut raw =
            json!({"id":"tagged", "descriptor":{"id":"tagged", "metadata":{"private":true}}});
        if let Some(tags) = &tags {
            raw["descriptor"]["tags"] = tags.clone();
        }
        let projected = project_detail("tagged", raw).unwrap();
        assert_eq!(projected["descriptor"].get("tags"), tags.as_ref());
        assert!(projected["descriptor"].get("metadata").is_none());
    }
}

#[test]
fn detail_rejects_invalid_descriptor_tags() {
    for tags in [
        json!(null),
        json!("tag"),
        json!([1]),
        json!([""]),
        json!(["x", "x"]),
        json!(["nul\0tag"]),
        json!(["é".repeat(33)]),
        json!((0..65).map(|i| i.to_string()).collect::<Vec<_>>()),
    ] {
        assert!(matches!(
            project_detail(
                "tagged",
                json!({"id":"tagged", "descriptor":{"id":"tagged", "tags":tags}})
            ),
            Err(DiscoveryError::InvalidProvider)
        ));
    }
}

#[test]
fn detail_source_format_preserves_optional_values_and_strips_private_provenance() {
    for value in [
        None,
        Some(json!(null)),
        Some(json!("")),
        Some(json!("agent-skill")),
        Some(json!("é".repeat(64))),
    ] {
        let mut provenance = json!({"sourceUri":"https://private.invalid", "metadata":{"secret":"hidden"}, "integrityEvidence":{"verified":true}});
        if let Some(value) = &value {
            provenance["originalFormat"] = value.clone();
            provenance["originalVersion"] = value.clone();
        }
        let raw = json!({"id":"format", "descriptor":{"id":"format"}, "provenance":provenance});
        let result = project_detail("format", raw.clone()).unwrap();
        assert_eq!(result["provenance"].get("originalFormat"), value.as_ref());
        assert_eq!(result["provenance"].get("originalVersion"), value.as_ref());
        assert_eq!(
            result["provenance"].as_object().unwrap().len(),
            if value.is_some() { 2 } else { 0 }
        );
        let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
        assert_eq!(
            merge_page(&mut pages, 0, 1).unwrap().items[0].get("provenance"),
            result.get("provenance")
        );
    }
    assert!(
        project_detail(
            "format",
            json!({"id":"format", "descriptor":{"id":"format"}})
        )
        .unwrap()
        .get("provenance")
        .is_none()
    );
}

#[test]
fn source_origin_is_allowlisted_and_preserved_in_list_and_detail() {
    for origin in [
        json!(null),
        json!("mcp-registry"),
        json!("acp-registry"),
        json!("ard"),
    ] {
        let raw = json!({"id":"origin", "descriptor":{"id":"origin"}, "sourceOrigin":origin});
        let detail = project_detail("origin", raw.clone()).unwrap();
        assert_eq!(detail["sourceOrigin"], origin);
        let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
        assert_eq!(
            merge_page(&mut pages, 0, 1).unwrap().items[0]["sourceOrigin"],
            origin
        );
    }
    for origin in [
        json!("github"),
        json!("https://private.invalid"),
        json!(""),
        json!(42),
        json!({}),
    ] {
        assert!(matches!(
            project_detail(
                "origin",
                json!({"id":"origin", "descriptor":{"id":"origin"}, "sourceOrigin":origin})
            ),
            Err(DiscoveryError::InvalidProvider)
        ));
    }
    assert!(
        project_detail(
            "origin",
            json!({"id":"origin", "descriptor":{"id":"origin"}})
        )
        .unwrap()
        .get("sourceOrigin")
        .is_none()
    );
}

#[test]
fn detail_source_format_rejects_unbounded_or_nontext_values() {
    for field in ["originalFormat", "originalVersion"] {
        for value in [
            json!(42),
            json!([]),
            json!({}),
            json!("é".repeat(65)),
            json!("nul\0text"),
        ] {
            let mut raw = json!({"id":"format", "descriptor":{"id":"format"}, "provenance":{}});
            raw["provenance"][field] = value;
            assert!(matches!(
                project_detail("format", raw.clone()),
                Err(DiscoveryError::InvalidProvider)
            ));
            let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
            assert_eq!(
                merge_page(&mut pages, 0, 1).unwrap_err(),
                DiscoveryError::InvalidProvider
            );
        }
    }
}

#[test]
fn list_and_detail_reject_invalid_file_counts() {
    for count in [
        json!(null),
        json!(-1),
        json!(1.5),
        json!("3"),
        json!({}),
        json!([]),
        json!(2001),
    ] {
        let raw = json!({"id": "files", "descriptor": {"id": "files"}, "currentRevision": {"fileCount": count}});
        assert_eq!(
            project_detail("files", raw.clone()),
            Err(DiscoveryError::InvalidProvider)
        );
        let mut pages = vec![ProviderPage::participating("alpha", vec![raw], None, None)];
        assert_eq!(
            merge_page(&mut pages, 0, 1).unwrap_err(),
            DiscoveryError::InvalidProvider
        );
    }
}

#[test]
fn list_and_detail_preserve_bounded_real_timestamps() {
    let raw = json!({
        "id": "dated", "descriptor": {"id": "dated"},
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": null,
        "firstSeenAt": "2026-09-08T00:00:00Z",
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
        assert_eq!(artifact["firstSeenAt"], "2026-09-08T00:00:00Z");
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
fn first_seen_is_absent_when_unknown_and_rejects_explicit_null() {
    let mut raw =
        json!({"id": "dated", "descriptor": {"id": "dated"}, "createdAt": "2020-01-01T00:00:00Z"});
    let detail = project_detail("dated", raw.clone()).unwrap();
    assert!(detail.get("firstSeenAt").is_none());
    let mut pages = vec![ProviderPage::participating(
        "alpha",
        vec![raw.clone()],
        None,
        None,
    )];
    assert!(
        merge_page(&mut pages, 0, 1).unwrap().items[0]
            .get("firstSeenAt")
            .is_none()
    );
    raw["firstSeenAt"] = json!(null);
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

#[test]
fn list_and_detail_reject_oversized_or_structured_timestamps() {
    for field in ["createdAt", "updatedAt", "authoredAt", "firstSeenAt"] {
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
#[test]
fn source_origin_request_scope_and_reply_are_strict() {
    use super::discovery::{DiscoveryRequest, SourceOrigin};
    for invalid in [json!("claude"), json!("MCP-REGISTRY"), json!(1), json!([])] {
        assert!(
            serde_json::from_value::<DiscoveryRequest>(json!({"sourceOrigin":invalid})).is_err()
        );
    }
    let unfiltered: DiscoveryRequest = serde_json::from_value(json!({})).unwrap();
    let filtered: DiscoveryRequest = serde_json::from_value(json!({"sourceOrigin":"ard"})).unwrap();
    assert_eq!(filtered.source_origin, Some(SourceOrigin::Ard));
    assert_eq!(
        super::discovery::page_contract(&unfiltered),
        "discovery/v1:50"
    );
    assert_ne!(
        super::discovery::page_contract(&unfiltered),
        super::discovery::page_contract(&filtered)
    );
    let mut other = filtered.clone();
    other.source_origin = Some(SourceOrigin::McpRegistry);
    assert_ne!(
        super::discovery::page_contract(&other),
        super::discovery::page_contract(&filtered)
    );

    let identity = |capability: Value| {
        super::provider::Identity::parse(json!({
        "contractVersion":"depot.discovery/v1","deploymentId":"deployment","deploymentEpoch":"boot",
        "authorityEpoch":"authority","listingEpoch":"listing","snapshotContinuations":true,"maxPageSize":200,
        "filters":{"sourceOrigin":capability}
    })).unwrap()
    };
    let capability = json!({"version":"source-origin/v1","values":["ard"]});
    let mut reply = super::provider::Reply {
        identity: identity(capability.clone()),
        result: json!({"sourceOrigin":"ard","artifacts":[{"id":"a","sourceOrigin":"ard"}]}),
    };
    assert!(super::discovery::validate_origin_reply(&reply, filtered.source_origin).is_ok());
    for wrong in [Value::Null, json!("mcp-registry"), json!(1)] {
        reply.result["sourceOrigin"] = wrong.clone();
        assert!(super::discovery::validate_origin_reply(&reply, filtered.source_origin).is_err());
        reply.result["sourceOrigin"] = json!("ard");
        reply.result["artifacts"][0]["sourceOrigin"] = wrong;
        assert!(super::discovery::validate_origin_reply(&reply, filtered.source_origin).is_err());
        reply.result["artifacts"][0]["sourceOrigin"] = json!("ard");
    }
    for capability in [
        Value::Null,
        json!({"version":"source-origin/v2","values":["ard"]}),
        json!({"version":"source-origin/v1","values":["mcp-registry"]}),
    ] {
        reply.identity = identity(capability);
        assert!(super::discovery::validate_origin_reply(&reply, filtered.source_origin).is_err());
        assert!(super::discovery::validate_origin_reply(&reply, None).is_ok());
    }
}
