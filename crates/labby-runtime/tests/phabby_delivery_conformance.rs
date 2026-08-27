use labby_runtime::phabby_delivery::{
    ApprovedDnsPolicy, ChunkManifest, ContractError, DeliveryErrorEnvelope, DeliveryReceipt,
    DeliveryRequest, DeliveryState, DownloadGrantClaims, IdentityLinkChallenge,
    IdentityLinkReceipt, ReceiptSummary, Validate, dns_policy_id, dns_policy_preimage,
    parse_canonical_json, parse_json,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

fn ts(value: &str) -> jiff::Timestamp {
    value.parse().unwrap()
}

fn success_summary(count: u32, rank: usize) -> ReceiptSummary {
    let reached = |milestone: usize| u32::from(rank >= milestone) * count;
    ReceiptSummary {
        requested: count,
        granted: reached(1),
        transferred: reached(2),
        verified: reached(3),
        stored: reached(4),
        materialized: reached(5),
        exposed: reached(6),
        activated: reached(7),
        incompatible: 0,
        partial: 0,
        cancelled: 0,
        failed: 0,
    }
}

const FIXTURES: &str = "../../../docs/contracts/phabby/fixtures";

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("../../../docs/contracts/phabby/fixtures/", $name)).as_slice()
    };
}

#[test]
fn consumes_all_versioned_golden_fixtures() {
    let challenge =
        parse_json::<IdentityLinkChallenge>(fixture!("identity-link-challenge.json")).unwrap();
    let link = parse_json::<IdentityLinkReceipt>(fixture!("identity-link-receipt.json")).unwrap();
    link.matches_challenge(&challenge).unwrap();

    let request = parse_json::<DeliveryRequest>(fixture!("delivery-request.json")).unwrap();
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    claims.matches_link(&link).unwrap();
    claims.matches_request(&request).unwrap();
    claims.matches_manifest(&manifest).unwrap();

    for bytes in [
        fixture!("delivery-receipt-activated.json"),
        fixture!("delivery-receipt-stored-activation-failed.json"),
    ] {
        parse_json::<DeliveryReceipt>(bytes)
            .unwrap()
            .matches_manifest(&manifest)
            .unwrap();
    }
    for bytes in [
        fixture!("delivery-error-expired-grant.json"),
        fixture!("delivery-error-replayed-grant.json"),
        fixture!("delivery-error-revoked-grant.json"),
        fixture!("delivery-error-wrong-target.json"),
    ] {
        parse_json::<DeliveryErrorEnvelope>(bytes).unwrap();
    }
    assert!(std::path::Path::new(FIXTURES).ends_with("docs/contracts/phabby/fixtures"));
}

#[test]
fn rejects_cross_authority_identity_revision_digest_and_audience() {
    let link = parse_json::<IdentityLinkReceipt>(fixture!("identity-link-receipt.json")).unwrap();
    let request = parse_json::<DeliveryRequest>(fixture!("delivery-request.json")).unwrap();
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();

    for mutate in [
        |value: &mut DownloadGrantClaims| value.sub = "acct_other".into(),
        |value: &mut DownloadGrantClaims| value.tenant_id = "ten_other".into(),
        |value: &mut DownloadGrantClaims| value.connection_id = "con_other".into(),
        |value: &mut DownloadGrantClaims| value.iss = "https://other.example.test".into(),
    ] {
        let mut changed = claims.clone();
        mutate(&mut changed);
        assert!(matches!(
            changed.matches_link(&link),
            Err(ContractError::IdentityMismatch(_))
        ));
    }
    let mut changed = claims.clone();
    changed.revision_id = "rev_other".into();
    assert!(matches!(
        changed.matches_request(&request),
        Err(ContractError::IdentityMismatch("revisionId"))
    ));
    let mut changed = claims.clone();
    changed.content_digest =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    assert!(matches!(
        changed.matches_manifest(&manifest),
        Err(ContractError::IdentityMismatch("contentDigest"))
    ));
    let mut changed = claims.clone();
    changed.aud = "labby:admin".into();
    assert!(changed.validate().is_err());
}

#[test]
fn rejects_expired_future_and_malformed_times_and_disallowed_origins() {
    let challenge =
        parse_json::<IdentityLinkChallenge>(fixture!("identity-link-challenge.json")).unwrap();
    assert!(
        challenge
            .validate_at(ts("2026-08-22T14:10:30Z"), 30)
            .is_err()
    );
    challenge
        .validate_at(ts("2026-08-22T14:10:29Z"), 30)
        .unwrap();
    let mut malformed = challenge.clone();
    malformed.expires_at = "not-a-time".into();
    assert!(malformed.validate().is_err());

    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    claims.validate_at(ts("2026-08-22T10:02:30Z"), 30).unwrap();
    assert!(claims.validate_at(ts("2026-08-22T10:05:30Z"), 30).is_err());
    assert!(claims.validate_at(ts("2026-08-22T09:59:29Z"), 30).is_err());
    let mut overflow = claims.clone();
    overflow.exp = u64::MAX;
    assert!(
        overflow
            .validate_at(ts("2026-08-22T10:02:30Z"), 30)
            .is_err()
    );
    for origin in [
        "https://user@example.test",
        "https://127.0.0.1",
        "https://[::1]",
        "https://depot.local",
    ] {
        let mut changed = claims.clone();
        changed.iss = origin.into();
        assert!(changed.validate().is_err(), "accepted {origin}");
    }
}

#[test]
fn dns_policy_binding_rejects_private_resolution_and_rebinding() {
    let challenge =
        parse_json::<IdentityLinkChallenge>(fixture!("identity-link-challenge.json")).unwrap();
    let link = parse_json::<IdentityLinkReceipt>(fixture!("identity-link-receipt.json")).unwrap();
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    let addresses = BTreeSet::from(["1.1.1.1".parse().unwrap()]);
    let approved = ApprovedDnsPolicy {
        id: dns_policy_id(&challenge.depot_origin, &addresses).unwrap(),
        depot_origin: challenge.depot_origin.clone(),
        resolved_addresses: addresses,
    };
    let mut link = link;
    link.dns_policy_id = approved.id.clone();
    let mut claims = claims;
    claims.dns_policy_id = approved.id.clone();
    link.matches_dns_policy(&approved).unwrap();
    claims.matches_dns_policy(&approved).unwrap();
    approved
        .validate_selected_address("https://depot.example.test", "1.1.1.1".parse().unwrap())
        .unwrap();
    assert!(
        approved
            .validate_selected_address("https://other.example.test", "1.1.1.1".parse().unwrap())
            .is_err()
    );
    assert!(
        approved
            .validate_selected_address("https://depot.example.test", "8.8.8.8".parse().unwrap())
            .is_err()
    );

    let private = ApprovedDnsPolicy {
        resolved_addresses: BTreeSet::from(["169.254.169.254".parse().unwrap()]),
        ..approved.clone()
    };
    assert!(private.validate().is_err());

    let rebound = ApprovedDnsPolicy {
        resolved_addresses: BTreeSet::from(["8.8.8.8".parse().unwrap()]),
        ..approved
    };
    assert!(rebound.validate().is_err());
    assert!(claims.matches_dns_policy(&rebound).is_err());
}

#[test]
fn signed_manifest_requires_canonical_bytes_and_exact_grant_binding() {
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    let canonical = labby_runtime::artifacts::canonical_json::to_canonical_vec(&manifest).unwrap();
    parse_canonical_json::<ChunkManifest>(&canonical).unwrap();
    assert_eq!(
        parse_canonical_json::<ChunkManifest>(fixture!("chunk-manifest.json")),
        Err(ContractError::NonCanonical)
    );
    let mut wrong_target = manifest.clone();
    wrong_target.target_id = "labby_other".into();
    assert_eq!(
        claims.matches_manifest(&wrong_target),
        Err(ContractError::IdentityMismatch("targetId"))
    );

    let mut request = parse_json::<DeliveryRequest>(fixture!("delivery-request.json")).unwrap();
    request.connection_id = "con_other".into();
    assert_eq!(
        claims.matches_request(&request),
        Err(ContractError::IdentityMismatch("connectionId"))
    );
}

#[test]
fn fails_closed_on_unknown_duplicate_unsafe_and_unbounded_data() {
    let mut request: serde_json::Value =
        serde_json::from_slice(fixture!("delivery-request.json")).unwrap();
    request
        .as_object_mut()
        .unwrap()
        .insert("futurePolicy".into(), json!(true));
    assert!(parse_json::<DeliveryRequest>(&serde_json::to_vec(&request).unwrap()).is_err());

    let duplicate = br#"{"schemaVersion":"dinglebear.depot-delivery/v1","schemaVersion":"dinglebear.depot-delivery/v1"}"#;
    assert!(parse_json::<DeliveryRequest>(duplicate).is_err());

    let mut manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    manifest.components[1].dependencies = vec![manifest.components[1].component_id.clone()];
    assert!(manifest.validate().is_err());
    manifest.components[1].dependencies.clear();
    manifest.components[1].path = "../escape".into();
    assert!(manifest.validate().is_err());

    let too_large = vec![b' '; 16 * 1024 * 1024 + 1];
    assert!(parse_json::<DeliveryRequest>(&too_large).is_err());
    let excessive_collection = serde_json::to_vec(&vec![0; 8_193]).unwrap();
    assert!(parse_json::<DeliveryRequest>(&excessive_collection).is_err());
    let excessive_string = serde_json::to_vec(&"x".repeat(16_385)).unwrap();
    assert!(parse_json::<DeliveryRequest>(&excessive_string).is_err());
}

#[test]
fn rejects_platform_alias_paths_and_nonexclusive_chunk_ownership() {
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    for path in [
        "C:/escape",
        "aux/file",
        "dir/con",
        "dir/CON.txt",
        "dir/COM1.log",
        "file.txt.",
        "file.txt ",
        "file:stream",
        "dir\\file",
    ] {
        let mut changed = manifest.clone();
        changed.components[0].path = path.into();
        assert!(changed.validate().is_err(), "accepted {path}");
    }
    let mut collision = manifest.clone();
    collision.components[1].path = "SKILL\u{212b}.md".into();
    collision.components[0].path = "skill\u{00c5}.md".into();
    assert!(collision.validate().is_err());

    let mut duplicate = manifest.clone();
    duplicate.components[1].chunks = duplicate.components[0].chunks.clone();
    assert!(duplicate.validate().is_err());
    let mut orphan = manifest;
    orphan.components[0].chunks.clear();
    orphan.components[0].chunks.push(1);
    assert!(orphan.validate().is_err());
}

#[test]
fn caps_clock_skew_and_rejects_secret_shaped_public_messages() {
    let challenge =
        parse_json::<IdentityLinkChallenge>(fixture!("identity-link-challenge.json")).unwrap();
    assert!(
        challenge
            .validate_at(ts("2026-08-22T14:10:00Z"), 31)
            .is_err()
    );
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    assert!(claims.validate_at(ts("2026-08-22T10:02:30Z"), 31).is_err());

    let mut receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-stored-activation-failed.json"))
            .unwrap();
    for secret in [
        "Bearer abc.def.ghi",
        "Cookie: sid=short",
        "Basic dXNlcjpwYXNz",
        "AKIAIOSFODNN7EXAMPLE",
        "hunter2",
    ] {
        receipt.components[1].message = Some(secret.into());
        assert!(receipt.validate().is_err(), "accepted {secret}");
    }
    let mut envelope =
        parse_json::<DeliveryErrorEnvelope>(fixture!("delivery-error-expired-grant.json")).unwrap();
    envelope.error.message = "access_token=abc".into();
    assert!(envelope.validate().is_err());
    envelope.error.message = "opaque ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 value".into();
    assert!(envelope.validate().is_err());
}

#[test]
fn dns_policy_rejects_ipv6_translation_and_reserved_ranges() {
    for address in [
        "::ffff:192.0.2.1",
        "64:ff9b::c000:201",
        "64:ff9b:1::c000:201",
        "100::1",
        "2001::1",
        "2001:db8::1",
        "2002:c000:0201::1",
        "3fff::1",
        "fc00::1",
        "fe80::1",
    ] {
        let addresses = BTreeSet::from([address.parse().unwrap()]);
        let policy = ApprovedDnsPolicy {
            id: dns_policy_id("https://depot.example.test", &addresses).unwrap(),
            depot_origin: "https://depot.example.test".into(),
            resolved_addresses: addresses,
        };
        assert!(policy.validate().is_err(), "accepted {address}");
    }
    let addresses = BTreeSet::from(["2606:4700:4700::1111".parse().unwrap()]);
    ApprovedDnsPolicy {
        id: dns_policy_id("https://depot.example.test", &addresses).unwrap(),
        depot_origin: "https://depot.example.test".into(),
        resolved_addresses: addresses,
    }
    .validate()
    .unwrap();
}

#[test]
fn receipt_binds_the_complete_delivery_chain() {
    let request = parse_json::<DeliveryRequest>(fixture!("delivery-request.json")).unwrap();
    let grant = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    let receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    receipt
        .matches_delivery_chain(&request, &grant, &manifest)
        .unwrap();
    let mut changed = request.clone();
    changed.resource.id = "art_other".into();
    assert!(
        receipt
            .matches_delivery_chain(&changed, &grant, &manifest)
            .is_err()
    );
    let mut malformed = grant.clone();
    malformed.target_id = "bad".into();
    assert!(
        receipt
            .matches_delivery_chain(&request, &malformed, &manifest)
            .is_err()
    );
    let mut wrong_correlation = request.clone();
    wrong_correlation.correlation_id = "cor_other".into();
    assert!(
        receipt
            .matches_delivery_chain(&wrong_correlation, &grant, &manifest)
            .is_err()
    );
    let mut wrong_tenant = grant.clone();
    wrong_tenant.tenant_id = "ten_other".into();
    assert!(
        receipt
            .matches_delivery_chain(&request, &wrong_tenant, &manifest)
            .is_err()
    );
}

#[test]
fn every_nonterminal_state_can_finish_with_explicit_preserved_progress() {
    let template =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    let successes = [
        DeliveryState::Requested,
        DeliveryState::Granted,
        DeliveryState::Transferred,
        DeliveryState::Verified,
        DeliveryState::Stored,
        DeliveryState::Materialized,
        DeliveryState::Exposed,
    ];
    for (rank, success) in successes.into_iter().enumerate() {
        let mut previous = template.clone();
        previous.receipt_id = format!("rcpt_previous_{rank}");
        previous.sequence = 1;
        previous.state = success;
        for component in &mut previous.components {
            component.state = success;
            component.completed_through = None;
        }
        let count = previous.components.len() as u32;
        previous.summary = success_summary(count, rank);
        previous.validate().unwrap();
        for terminal in [
            DeliveryState::Incompatible,
            DeliveryState::Cancelled,
            DeliveryState::Failed,
        ] {
            let mut next = previous.clone();
            next.receipt_id = format!("rcpt_terminal_{rank}_{terminal:?}");
            next.sequence = 2;
            next.occurred_at = "2026-08-22T15:00:00Z".into();
            next.state = terminal;
            for component in &mut next.components {
                component.state = terminal;
                component.completed_through = Some(success);
                if terminal != DeliveryState::Cancelled {
                    component.stage = Some("delivery".into());
                    component.code = Some("terminal_outcome".into());
                    component.retryable = Some(false);
                }
            }
            next.summary.partial = count;
            match terminal {
                DeliveryState::Incompatible => next.summary.incompatible = count,
                DeliveryState::Cancelled => next.summary.cancelled = count,
                DeliveryState::Failed => next.summary.failed = count,
                _ => unreachable!(),
            }
            next.validate().unwrap();
            next.follows(&previous).unwrap();
        }
    }
}

#[test]
fn dns_policy_id_uses_the_closed_v1_canonical_preimage() {
    let addresses = BTreeSet::from([
        "1.1.1.1".parse().unwrap(),
        "2606:4700:4700::1111".parse().unwrap(),
    ]);
    assert_eq!(
        dns_policy_id("https://DEPOT.EXAMPLE.TEST.:443", &addresses).unwrap(),
        dns_policy_id("https://depot.example.test", &addresses).unwrap()
    );
    for noncanonical in [
        "https://DEPOT.EXAMPLE.TEST",
        "https://depot.example.test.",
        "https://depot.example.test:443",
    ] {
        let policy = ApprovedDnsPolicy {
            id: dns_policy_id(noncanonical, &addresses).unwrap(),
            depot_origin: noncanonical.into(),
            resolved_addresses: addresses.clone(),
        };
        assert!(policy.validate().is_err(), "accepted {noncanonical}");
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DnsPolicyVectors {
    schema_version: String,
    vectors: Vec<DnsPolicyVector>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DnsPolicyVector {
    name: String,
    origin: String,
    addresses: Vec<String>,
    preimage: String,
    dns_policy_id: String,
}

#[test]
fn consumes_all_canonical_dns_policy_vectors() {
    let fixture: DnsPolicyVectors =
        serde_json::from_slice(fixture!("dns-policy-vectors.json")).unwrap();
    assert_eq!(
        fixture.schema_version,
        "dinglebear.depot-dns-policy-v1/vectors"
    );
    assert_eq!(fixture.vectors.len(), 3);
    for vector in fixture.vectors {
        let addresses = vector
            .addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            dns_policy_preimage(&vector.origin, &addresses).unwrap(),
            vector.preimage.as_bytes(),
            "preimage mismatch for {}",
            vector.name
        );
        assert_eq!(
            dns_policy_id(&vector.origin, &addresses).unwrap(),
            vector.dns_policy_id,
            "ID mismatch for {}",
            vector.name
        );
        ApprovedDnsPolicy {
            id: vector.dns_policy_id,
            depot_origin: vector.origin,
            resolved_addresses: addresses,
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn rejects_manifest_path_graph_limit_and_overflow_attacks() {
    let claims = parse_json::<DownloadGrantClaims>(fixture!("download-grant-claims.json")).unwrap();
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    for path in [
        "/v1/deliveries/del_other/chunks/0",
        "/v1/deliveries/del_01K35DWEEVDTNC2FW4Z6QB8MM8/chunks/1",
        "/v1/deliveries/del_01K35DWEEVDTNC2FW4Z6QB8MM8/chunks/0/extra",
        "https://depot.example.test/chunk",
    ] {
        let mut changed = manifest.clone();
        changed.chunks[0].download_path = path.into();
        assert!(changed.validate().is_err(), "accepted {path}");
    }
    let mut changed = manifest.clone();
    changed.components[0].dependencies = vec![changed.components[1].component_id.clone()];
    assert!(changed.validate().is_err());
    let mut changed = manifest.clone();
    changed.total_compressed_bytes = u64::MAX;
    assert!(changed.validate().is_err());
    let mut changed = manifest.clone();
    changed.revision_id = "rev_other".into();
    assert!(matches!(
        claims.matches_manifest(&changed),
        Err(ContractError::IdentityMismatch("revisionId"))
    ));
}

#[test]
fn receipt_rejects_state_detail_and_count_mismatches() {
    let mut receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-stored-activation-failed.json"))
            .unwrap();
    receipt.components[1].code = None;
    assert!(receipt.validate().is_err());
    receipt.components[1].code = Some("target_requirement_unsatisfied".into());
    receipt.components[1].completed_through = None;
    assert!(receipt.validate().is_err());
    receipt.components[1].completed_through = Some(DeliveryState::Materialized);
    receipt.summary.activated = 99;
    assert!(receipt.validate().is_err());

    let mut receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    receipt.summary.verified = 1;
    assert!(receipt.validate().is_err());
    let mut receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    receipt.components[0].state = DeliveryState::Stored;
    assert!(receipt.validate().is_err());
}

#[test]
fn activation_failure_distinguishes_materialized_from_exposed_progress() {
    let materialized =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-stored-activation-failed.json"))
            .unwrap();
    assert_eq!(
        materialized.components[1].completed_through,
        Some(DeliveryState::Materialized)
    );
    let mut exposed = materialized.clone();
    exposed.components[1].completed_through = Some(DeliveryState::Exposed);
    exposed.summary.exposed = 1;
    exposed.validate().unwrap();
    assert_ne!(materialized.summary, exposed.summary);
}

#[test]
fn receipts_reject_component_summary_lies_and_skipped_aggregate_transitions() {
    let mut receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    receipt.state = DeliveryState::Stored;
    assert!(receipt.validate().is_err());

    let count = receipt.components.len() as u32;
    for component in &mut receipt.components {
        component.state = DeliveryState::Requested;
    }
    receipt.state = DeliveryState::Requested;
    receipt.summary = success_summary(count, 0);
    receipt.validate().unwrap();

    let mut skipped = receipt.clone();
    skipped.receipt_id = "rcpt_skipped_transition".into();
    skipped.sequence += 1;
    skipped.occurred_at = "2026-08-22T14:11:00Z".into();
    for component in &mut skipped.components {
        component.state = DeliveryState::Transferred;
    }
    skipped.state = DeliveryState::Transferred;
    skipped.summary.granted = count;
    skipped.summary.transferred = count;
    skipped.validate().unwrap();
    assert!(skipped.follows(&receipt).is_err());
}

#[test]
fn partial_receipts_preserve_progress_and_resume_only_at_same_or_adjacent_stage() {
    let mut transferred =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    transferred.receipt_id = "rcpt_transferred".into();
    transferred.sequence = 1;
    transferred.state = DeliveryState::Transferred;
    for component in &mut transferred.components {
        component.state = DeliveryState::Transferred;
        component.completed_through = None;
    }
    let count = transferred.components.len() as u32;
    transferred.summary = success_summary(count, 2);
    transferred.validate().unwrap();

    let mut partial = transferred.clone();
    partial.receipt_id = "rcpt_partial".into();
    partial.sequence = 2;
    partial.occurred_at = "2026-08-22T14:11:00Z".into();
    partial.state = DeliveryState::Partial;
    for component in &mut partial.components {
        component.state = DeliveryState::Partial;
        component.completed_through = Some(DeliveryState::Transferred);
    }
    partial.summary.partial = count;
    partial.validate().unwrap();
    partial.follows(&transferred).unwrap();

    let mut resumed = transferred.clone();
    resumed.receipt_id = "rcpt_resumed".into();
    resumed.sequence = 3;
    resumed.occurred_at = "2026-08-22T14:12:00Z".into();
    resumed.follows(&partial).unwrap();

    let mut verified = resumed.clone();
    verified.receipt_id = "rcpt_verified".into();
    verified.sequence = 4;
    verified.occurred_at = "2026-08-22T14:13:00Z".into();
    verified.state = DeliveryState::Verified;
    for component in &mut verified.components {
        component.state = DeliveryState::Verified;
    }
    verified.summary.verified = count;
    verified.validate().unwrap();
    verified.follows(&partial).unwrap();

    let mut skipped = partial.clone();
    skipped.receipt_id = "rcpt_partial_skipped".into();
    skipped.sequence = 3;
    skipped.occurred_at = "2026-08-22T14:12:00Z".into();
    for component in &mut skipped.components {
        component.completed_through = Some(DeliveryState::Stored);
    }
    skipped.summary.verified = count;
    skipped.summary.stored = count;
    skipped.validate().unwrap();
    assert!(skipped.follows(&partial).is_err());
}

#[test]
fn receipts_reject_binding_sequence_time_and_state_regressions() {
    let previous =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    let mut next = previous.clone();
    next.sequence += 1;
    assert!(next.follows(&previous).is_err());

    let previous =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-stored-activation-failed.json"))
            .unwrap();
    for mutate in [
        |value: &mut DeliveryReceipt| value.delivery_id = "del_other".into(),
        |value: &mut DeliveryReceipt| value.tenant_id = "ten_other".into(),
        |value: &mut DeliveryReceipt| value.target_id = "labby_other".into(),
        |value: &mut DeliveryReceipt| value.resource.revision_id = "rev_other".into(),
    ] {
        let mut next = previous.clone();
        next.sequence += 1;
        mutate(&mut next);
        assert!(matches!(
            next.follows(&previous),
            Err(ContractError::IdentityMismatch(_))
        ));
    }
    let mut next = previous.clone();
    next.sequence = previous.sequence;
    assert!(next.follows(&previous).is_err());
    let mut next = previous.clone();
    next.sequence += 1;
    next.occurred_at = "2026-08-22T14:00:00Z".into();
    assert!(next.follows(&previous).is_err());
    let mut next = previous.clone();
    next.sequence += 1;
    next.components[0].state = DeliveryState::Verified;
    assert!(next.follows(&previous).is_err());
}

#[test]
fn receipts_bind_exact_component_sets_to_manifest_and_prior_snapshot() {
    let manifest = parse_json::<ChunkManifest>(fixture!("chunk-manifest.json")).unwrap();
    let receipt =
        parse_json::<DeliveryReceipt>(fixture!("delivery-receipt-activated.json")).unwrap();
    receipt.matches_manifest(&manifest).unwrap();

    let mut added = receipt.clone();
    let mut extra = added.components[0].clone();
    extra.component_id = "cmp_extra".into();
    added.components.push(extra);
    added.summary = success_summary(added.components.len() as u32, 7);
    assert!(added.validate().is_ok());
    assert!(added.matches_manifest(&manifest).is_err());

    let mut removed = receipt.clone();
    removed.components.pop();
    removed.summary = success_summary(removed.components.len() as u32, 7);
    assert!(removed.validate().is_ok());
    assert!(removed.matches_manifest(&manifest).is_err());

    let mut substituted = receipt.clone();
    substituted.components[0].component_id = "cmp_substitute".into();
    assert!(substituted.validate().is_ok());
    assert!(substituted.matches_manifest(&manifest).is_err());

    let mut previous = receipt;
    previous.state = DeliveryState::Requested;
    for component in &mut previous.components {
        component.state = DeliveryState::Requested;
    }
    let count = previous.components.len() as u32;
    previous.summary = success_summary(count, 0);
    for mut next in [added, removed, substituted] {
        next.state = DeliveryState::Requested;
        for component in &mut next.components {
            component.state = DeliveryState::Requested;
        }
        next.summary = success_summary(next.components.len() as u32, 0);
        next.sequence = previous.sequence + 1;
        next.receipt_id = "rcpt_component_set_change".into();
        assert!(next.validate().is_ok());
        assert!(next.follows(&previous).is_err());
    }
}
