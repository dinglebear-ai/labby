use serde_json::Value;

const MANIFEST: &str =
    include_str!("../../../docs/contracts/fixtures/depot-control-plane/compatibility-v1.json");

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
    assert_eq!(value["flows"]["sendToLabby"]["status"], "deferred");
    assert_eq!(value["flows"]["sendToLabby"]["exactExport"], false);
    assert!(value["limits"]["artifactPage"].as_u64().unwrap() <= 200);
    assert!(
        value["limits"]["streamConcurrency"].as_u64().unwrap()
            < value["limits"]["interactiveConcurrency"].as_u64().unwrap()
    );
}
