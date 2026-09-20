use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use verify_core::InvariantId;

/// Maximum encoded input accepted by the scenario loader (one MiB).
pub const MAX_SCENARIO_BYTES: usize = 1_048_576;
/// Maximum transitions per scenario, before target decoding or execution.
pub const MAX_STEPS: usize = 10_000;

/// Provenance only; it never selects a replay implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    /// Stateright model checker.
    Stateright,
    /// Kani bounded verifier.
    Kani,
    /// Loom concurrency explorer.
    Loom,
    /// Shuttle schedule explorer.
    Shuttle,
    /// Alloy analyzer.
    Alloy,
    /// TLA+ backend.
    Tla,
    /// Fuzzer or property test.
    Fuzz,
    /// Redacted production incident.
    Incident,
    /// Hand-authored trace.
    Manual,
}

/// Discovery metadata, retained independently of scenario identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// How the trace was discovered.
    pub kind: OriginKind,
    /// Actual discovery tool version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
    /// Caller-provided discovery timestamp; informational, not interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_at: Option<String>,
    /// Reproduction seed, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Issue/incident reference; callers must redact sensitive data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Backend discovery bound; no nested payloads or floating-point ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Bound {
    /// Signed integer bound.
    Integer(i64),
    /// Nonnegative bound larger than i64::MAX.
    Unsigned(u64),
    /// Named or symbolic bound.
    Text(String),
    /// Boolean exploration setting.
    Boolean(bool),
}

/// Trace expectation, never a claim of universal verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Expectation {
    /// Reproduce a concrete violation against the specified target.
    InvariantViolated,
    /// A pinned golden trace should remain valid.
    InvariantHolds,
}

/// Reproduction lifecycle; only active scenarios gate a replay lane.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// CI gates on matching the expectation.
    #[default]
    Active,
    /// Nondeterministic evidence, reported without gating.
    Quarantined,
    /// Evidence awaiting reproduction, reported without gating.
    Unreproduced,
}

/// Raw envelope. Call `validate` before replay or corpus insertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Envelope version, currently one.
    #[schemars(range(min = 1, max = 1))]
    pub schema: u32,
    /// Adopting project identity.
    #[schemars(length(min = 1, max = 256))]
    pub project: String,
    /// Target registry key within the project.
    #[schemars(length(min = 1, max = 256))]
    pub model: String,
    /// Stable catalogued invariant.
    #[schemars(length(max = 128))]
    pub invariant: InvariantId,
    /// Retained discovery provenance.
    pub origin: Origin,
    /// Discovery bounds, not a finite-trace proof claim.
    #[serde(default)]
    pub bounds: BTreeMap<String, Bound>,
    /// Opaque object; missing means {}, while null is rejected by deserialization.
    #[serde(default)]
    pub initial: Map<String, Value>,
    /// Opaque transitions, interpreted only by the registered target.
    #[schemars(length(max = 10000))]
    pub steps: Vec<Value>,
    /// Required trace-wide result.
    pub expect: Expectation,
    /// Independent gating/reproduction status.
    #[serde(default)]
    pub status: Status,
    /// Optional supplied identity; validation rejects stale or forged values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = "^b3:[0-9a-f]{64}$"))]
    pub fingerprint: Option<String>,
}

/// Invalid envelope, distinct from a modeled violation.
#[derive(Debug, Error)]
pub enum EnvelopeError {
    /// Malformed JSON or wire shape.
    #[error("invalid scenario JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Required identity/version/collection constraint failed.
    #[error("invalid scenario field: {0}")]
    Invalid(&'static str),
    /// Encoded input or constructed envelope is too large.
    #[error("scenario exceeds the one MiB input limit")]
    TooLarge,
    /// Supplied content hash does not describe this trace.
    #[error("scenario fingerprint does not match its content")]
    FingerprintMismatch,
}

/// Immutable validated envelope with a recomputed content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedScenario(Scenario);

impl Scenario {
    /// Bounded JSON parsing followed by semantic validation.
    pub fn from_json(input: &str) -> Result<ValidatedScenario, EnvelopeError> {
        if input.len() > MAX_SCENARIO_BYTES {
            return Err(EnvelopeError::TooLarge);
        }
        let value = serde_json::from_str::<crate::strict_json::UniqueValue>(input)?.0;
        serde_json::from_value::<Self>(value)?.validate()
    }

    /// Validate constructed data too, then seal with its content fingerprint.
    pub fn validate(mut self) -> Result<ValidatedScenario, EnvelopeError> {
        if self.schema != 1 {
            return Err(EnvelopeError::Invalid("schema"));
        }
        for (name, value) in [("project", &self.project), ("model", &self.model)] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err(EnvelopeError::Invalid(name));
            }
        }
        if self.steps.len() > MAX_STEPS {
            return Err(EnvelopeError::Invalid("steps"));
        }
        if self.invariant.as_str().len() > 128 {
            return Err(EnvelopeError::Invalid("invariant"));
        }
        if serde_json::to_vec(&self)?.len() > MAX_SCENARIO_BYTES {
            return Err(EnvelopeError::TooLarge);
        }
        let fingerprint = crate::content_fingerprint(&self);
        if self
            .fingerprint
            .as_ref()
            .is_some_and(|supplied| supplied != &fingerprint)
        {
            return Err(EnvelopeError::FingerprintMismatch);
        }
        self.fingerprint = Some(fingerprint);
        // Reserve the newline used by corpus publication as well as the seal.
        if serde_json::to_vec(&self)?.len() + 1 > MAX_SCENARIO_BYTES {
            return Err(EnvelopeError::TooLarge);
        }
        Ok(ValidatedScenario(self))
    }
}

impl ValidatedScenario {
    /// Borrow the immutable envelope.
    pub fn scenario(&self) -> &Scenario {
        &self.0
    }

    /// Canonical content identity, always populated after validation.
    pub fn fingerprint(&self) -> String {
        crate::content_fingerprint(&self.0)
    }

    /// Safe relative corpus path: encoded project/model plus invariant and hash.
    /// No user-supplied identity is interpreted as a filesystem path.
    pub fn corpus_path(&self) -> PathBuf {
        PathBuf::from(component(&self.0.project))
            .join(component(&self.0.model))
            .join(format!(
                "{}-{}.json",
                self.0.invariant,
                &self.fingerprint()[3..]
            ))
    }
}

fn component(value: &str) -> String {
    let encoded: String = value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    if encoded.len() > 120 {
        format!(
            "{}-{}",
            &encoded[..48],
            blake3::hash(value.as_bytes()).to_hex()
        )
    } else {
        encoded
    }
}

/// Generate the committed wire schema from the deserialized Rust types.
pub fn scenario_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(Scenario);
    schema.insert(
        "$id".into(),
        "https://dinglebear.ai/schemas/verify/scenario/1".into(),
    );
    schema
}
