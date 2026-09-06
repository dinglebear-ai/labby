use serde_json::Value;

const MANIFEST: &str =
    include_str!("../../../docs/contracts/fixtures/depot-control-plane/compatibility-v1.json");
const FEDERATED: &str =
    include_str!("../../../docs/contracts/fixtures/depot-control-plane/compatibility-v2.json");

#[test]
fn operational_depot_contract_is_fail_closed_and_bounded() {
    let value: Value = serde_json::from_str(MANIFEST).expect("valid compatibility manifest");
    assert_eq!(value["schemaVersion"], "labby.depot-compatibility/v1");
    assert_eq!(value["mountPolicy"]["oauthBrowserSession"], "supported");
    for mode in [
        "staticBearerBrowser",
        "webUiAuthDisabled",
        "noAuth",
        "syntheticDevelopment",
    ] {
        assert_eq!(value["mountPolicy"][mode], "disabled", "{mode}");
    }
    assert_eq!(value["flows"]["bazaarBrowse"]["status"], "supported");
    assert_eq!(value["flows"]["sendToLabby"]["status"], "supported");
    assert_eq!(value["flows"]["sendToLabby"]["exactExport"], true);
    assert_eq!(
        value["flows"]["sendToLabby"]["operations"],
        serde_json::json!(["artifacts.import"])
    );
    assert_eq!(
        value["flows"]["browserCredentialAdministration"]["status"],
        "supported"
    );
    assert_eq!(value["flows"]["maintenance"]["status"], "supported");
    assert!(value["limits"]["artifactPage"].as_u64().unwrap() <= 200);
    assert!(
        value["limits"]["streamConcurrency"].as_u64().unwrap()
            < value["limits"]["interactiveConcurrency"].as_u64().unwrap()
    );
}

#[test]
fn federated_discovery_contract_is_provider_qualified_and_bounded() {
    let value: Value = serde_json::from_str(FEDERATED).expect("valid federated manifest");
    assert_eq!(value["schemaVersion"], "labby.depot-compatibility/v2");
    assert_eq!(value["identity"]["providerQualified"], true);
    assert_eq!(value["identity"]["rawArtifactIdRequired"], true);
    assert_eq!(value["cursor"]["opaqueRandomBits"], 256);
    assert_eq!(value["cursor"]["replayTransitions"], 2);
    assert!(value["limits"]["artifactPage"].as_u64().unwrap() <= 200);
    assert!(value["limits"]["responseBytes"].as_u64().unwrap() <= 1_048_576);
}
