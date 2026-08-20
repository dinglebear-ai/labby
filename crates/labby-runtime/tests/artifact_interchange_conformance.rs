use labby_runtime::artifacts::canonical_json;
use labby_runtime::artifacts::{ARTIFACT_INTERCHANGE_SCHEMA, ArtifactInterchange};

const FIXTURE: &[u8] = include_bytes!("fixtures/artifact-interchange-v1.json");

#[test]
fn depot_frozen_v1_fixture_round_trips_byte_canonically() {
    let interchange: ArtifactInterchange = serde_json::from_slice(FIXTURE).expect("parse fixture");
    assert_eq!(interchange.schema_version, ARTIFACT_INTERCHANGE_SCHEMA);
    interchange.validate().expect("validate frozen v1 fixture");
    let encoded = canonical_json::to_canonical_vec(&interchange).expect("canonical JSON");
    assert_eq!(encoded, FIXTURE);
    let reparsed: ArtifactInterchange = serde_json::from_slice(&encoded).expect("reparse");
    assert_eq!(reparsed, interchange);
}

#[test]
fn frozen_fixture_revision_digest_matches_cross_runtime_contract() {
    let interchange: ArtifactInterchange = serde_json::from_slice(FIXTURE).expect("parse fixture");
    assert_eq!(
        canonical_json::digest(&interchange.revision.components).expect("component digest"),
        "sha256:feda49490988a21b01ea9d6548f2c893a7cea6c4e9834322985c28d82280c13f"
    );
}
