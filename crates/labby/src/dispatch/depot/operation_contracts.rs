//! Labby-pinned Depot operation contracts.

use std::{collections::HashMap, sync::OnceLock};

use serde::Deserialize;

const DEPOT_OPERATIONS_GOLDEN: &str =
    include_str!("../../../../../docs/contracts/fixtures/depot-control-plane/operations-v1.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PinnedOperationContract {
    pub name: String,
    pub schema_fingerprint: String,
    pub required_scope: String,
    pub transport_available: bool,
    pub annotations: PinnedAnnotations,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PinnedAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
}

#[derive(Deserialize)]
struct Fixture {
    operations: Vec<PinnedOperationContract>,
}

/// Return the complete policy and schema contract admitted by this Labby build.
pub(super) fn expected_contract(name: &str) -> Option<&'static PinnedOperationContract> {
    static CONTRACTS: OnceLock<HashMap<String, PinnedOperationContract>> = OnceLock::new();
    CONTRACTS
        .get_or_init(|| {
            let fixture: Fixture = serde_json::from_str(DEPOT_OPERATIONS_GOLDEN)
                .expect("embedded Depot operations fixture must be valid");
            fixture
                .operations
                .into_iter()
                .map(|contract| (contract.name.clone(), contract))
                .collect()
        })
        .get(name)
}

/// Return the input-schema fingerprint pinned for a Depot operation.
#[cfg(test)]
pub(super) fn expected_schema_fingerprint(name: &str) -> Option<&'static str> {
    expected_contract(name).map(|contract| contract.schema_fingerprint.as_str())
}
