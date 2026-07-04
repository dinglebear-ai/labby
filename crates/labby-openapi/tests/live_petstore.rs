//! LIVE end-to-end smoke test against the public Swagger Petstore
//! (`https://petstore3.swagger.io`). Exercises the REAL production path —
//! SSRF-validated spec fetch through the hardened client, `rmcp-openapi`
//! parse, deny-by-default allowlist, and a hardened outbound dispatch — against
//! a real public OpenAPI document and live endpoint.
//!
//! Network-gated: marked `#[ignore]` per the repo convention (CI-safe). Run with:
//!   cargo test -p labby-openapi --test live_petstore -- --ignored --nocapture
//! or
//!   cargo nextest run -p labby-openapi --run-ignored ignored-only -E 'test(petstore)'

use std::time::Duration;

use labby_openapi::config::{OpenApiProviderConfig, OpenApiSpecConfig, SpecSource};
use labby_openapi::{OpenApiRegistry, dispatch_openapi_call};

#[tokio::test]
#[ignore = "network: hits the public Swagger Petstore"]
async fn petstore_live_load_and_dispatch() {
    let cfg = OpenApiProviderConfig {
        specs: vec![OpenApiSpecConfig {
            label: "petstore".into(),
            spec_source: SpecSource::Url(
                "https://petstore3.swagger.io/api/v3/openapi.json"
                    .parse()
                    .unwrap(),
            ),
            base_url: "https://petstore3.swagger.io/api/v3".parse().unwrap(),
            // Deny-by-default: only this one GET operation is dispatchable.
            allowed_operations: vec!["findPetsByStatus".into()],
            credential: None,
        }],
    };

    // Real fetch (SSRF-validated hardened client) + rmcp-openapi parse + allowlist.
    let registry = OpenApiRegistry::load(cfg, Duration::from_secs(10)).await;
    assert!(
        registry.labels().contains(&"petstore".to_string()),
        "petstore spec should load (labels: {:?})",
        registry.labels()
    );

    // Allowlisted operation is present; a non-allowlisted one is not.
    registry
        .operation("petstore", "findPetsByStatus")
        .expect("allowlisted operation present after load");
    assert_eq!(
        registry
            .operation("petstore", "getInventory")
            .unwrap_err()
            .kind(),
        "unknown_action",
        "non-allowlisted operation must be denied"
    );

    // Real outbound dispatch through the hardened, per-request-pinned client.
    let client = labby_openapi::http::build_dispatch_client().expect("build dispatch client");
    let out = dispatch_openapi_call(
        &registry,
        &client,
        "petstore",
        "findPetsByStatus",
        serde_json::json!({ "status": "available" }),
    )
    .await
    .expect("live dispatch to petstore should succeed");

    // Real API returns a JSON array of pets; each has an `id` and `status`.
    let pets = out
        .as_array()
        .expect("findPetsByStatus returns a JSON array");
    assert!(!pets.is_empty(), "expected at least one available pet");
    assert!(
        pets.iter()
            .all(|p| p.get("id").is_some() && p.get("status").is_some()),
        "each pet should have id + status"
    );
    eprintln!(
        "LIVE SMOKE OK: fetched + parsed petstore spec, dispatched findPetsByStatus, got {} pets",
        pets.len()
    );
}

/// The SSRF guard must reject a private base URL even for a live-looking spec —
/// confirms the production dispatch path fails closed (no network needed).
#[tokio::test]
async fn private_base_url_is_rejected_without_network() {
    let cfg = OpenApiProviderConfig {
        specs: vec![OpenApiSpecConfig {
            label: "internal".into(),
            spec_source: SpecSource::Url("https://10.0.0.5/openapi.json".parse().unwrap()),
            base_url: "https://10.0.0.5/api".parse().unwrap(),
            allowed_operations: vec!["anything".into()],
            credential: None,
        }],
    };
    // Load must omit the RFC1918 spec (degraded boot) — never register it.
    let registry = OpenApiRegistry::load(cfg, Duration::from_secs(3)).await;
    assert!(
        !registry.labels().contains(&"internal".to_string()),
        "a private-IP spec must be SSRF-rejected at load, not registered"
    );
}
