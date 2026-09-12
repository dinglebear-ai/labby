//! The scenario envelope: a portable, replayable trace.
//!
//! The envelope is project-agnostic. `initial` and `steps` are opaque here and
//! interpreted only by the adopting project's `ScenarioTarget`, which is what
//! lets one format carry counterexamples from a model checker, a fuzzer, and a
//! production incident alike.
//!
//! # Two axes, deliberately separate
//!
//! [`Expect`] is what replay *should* observe. [`ScenarioStatus`] is whether it
//! does. Collapsing them into one enum makes every incident-derived scenario an
//! instant CI failure the moment it is committed, which would kill the most
//! valuable source of scenarios the toolkit has.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use verify_core::InvariantId;

/// Schema version of the scenario format.
pub const SCENARIO_SCHEMA: u32 = 1;

/// A replayable trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema: u32,
    pub project: String,
    pub model: String,
    pub invariant: InvariantId,
    pub origin: Origin,
    /// Backend-reported bounds under which this trace was found. Free-form and
    /// retained so a report can stay honest about what was actually explored.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub bounds: Map<String, Value>,
    /// Opaque initial state. Absent is normalized to `{}` on load, so a target's
    /// `init` never has to distinguish absent from null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Map<String, Value>>,
    /// Opaque steps, deserialized into the target's `Step` type at replay.
    #[serde(default)]
    pub steps: Vec<Value>,
    pub expect: Expect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub status: ScenarioStatus,
}

impl Scenario {
    /// The initial state to hand a target, with absent normalized to `{}`.
    pub fn initial_value(&self) -> Value {
        Value::Object(self.initial.clone().unwrap_or_default())
    }

    /// Whether a mismatch between `expect` and replay should fail CI.
    ///
    /// Only `active` scenarios gate. Quarantined and unreproduced ones are
    /// reported and never fail, which is what lets them be committed at all.
    pub const fn gates_ci(&self) -> bool {
        matches!(self.status, ScenarioStatus::Active)
    }
}

/// What replay should observe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// A regression scenario reproducing a bug.
    InvariantViolated,
    /// A golden trace pinned against regression.
    InvariantHolds,
}

/// Whether replay currently observes what `expect` says it should.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    /// Replay matches `expect`. Gates CI.
    #[default]
    Active,
    /// Failed the determinism check. Reported, never gating.
    Quarantined,
    /// Committed as evidence — typically incident-derived — where replay does
    /// not match `expect`. A model gap, recorded rather than discarded.
    Unreproduced,
}

/// Where a scenario came from. Provenance is retained; it never changes replay
/// semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub kind: OriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Issue, PR, or incident reference. Never a raw log payload: this file is
    /// committed, and incident-derived scenarios are redacted by construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    Stateright,
    Kani,
    Loom,
    Shuttle,
    Alloy,
    Tla,
    Fuzz,
    Incident,
    Manual,
}

/// A scenario that could not be loaded.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScenarioLoadError {
    #[error("scenario is not valid JSON for this schema: {message}")]
    Malformed { message: String },
    #[error("scenario declares schema {found}, but this tool understands {SCENARIO_SCHEMA}")]
    Schema { found: u32 },
    #[error("scenario `initial` must be an object or absent, not null or a scalar")]
    InitialNotAnObject,
}

impl Scenario {
    /// Parse a scenario from JSON text, rejecting a literal `null` initial
    /// rather than coercing it.
    pub fn parse(text: &str) -> Result<Self, ScenarioLoadError> {
        let raw: Value =
            serde_json::from_str(text).map_err(|source| ScenarioLoadError::Malformed {
                message: source.to_string(),
            })?;
        // serde would happily read `"initial": null` as None. That conflates a
        // deliberate null with an absent key, and the spec rejects the former.
        if matches!(raw.get("initial"), Some(Value::Null)) {
            return Err(ScenarioLoadError::InitialNotAnObject);
        }
        if raw.get("initial").is_some_and(|value| !value.is_object()) {
            return Err(ScenarioLoadError::InitialNotAnObject);
        }
        let scenario: Self =
            serde_json::from_value(raw).map_err(|source| ScenarioLoadError::Malformed {
                message: source.to_string(),
            })?;
        if scenario.schema != SCENARIO_SCHEMA {
            return Err(ScenarioLoadError::Schema {
                found: scenario.schema,
            });
        }
        Ok(scenario)
    }
}
