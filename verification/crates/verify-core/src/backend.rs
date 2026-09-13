use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BackendId, InvariantId, Kind};

/// Declared backend capabilities, independent of whether its executable exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Property categories supported by the adapter.
    pub kinds: BTreeSet<Kind>,
    /// The adapter can express the fairness assumptions needed for liveness.
    pub fairness: bool,
    /// The adapter explores concurrent schedules.
    pub concurrency: bool,
    /// Results are limited to explicitly reported finite bounds.
    pub bounded: bool,
}

impl Capabilities {
    /// Whether a catalog binding can legitimately be accepted.
    pub fn supports(&self, kind: Kind) -> bool {
        self.kinds.contains(&kind) && (kind != Kind::Liveness || self.fairness)
    }
}

/// Tool availability is separate from capability/handle registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum Availability {
    /// The adapter is ready for an explicitly requested execution.
    Ready {},
    /// Execution is unavailable, but configuration can still be validated.
    Missing {
        /// A missing tool or environment prerequisite.
        reason: String,
    },
}

/// Backend-reported search/proof limits, retained verbatim as structured values.
pub type Bounds = BTreeMap<String, serde_json::Value>;

/// Honest backend result vocabulary. Replay expectations are a separate axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
pub enum Verdict {
    /// Property discharged for the adapter's stated semantic domain.
    Verified {},
    /// A concrete violation, possibly without a projectable trace.
    Falsified {
        /// Redacted violation diagnostic.
        reason: String,
    },
    /// Checked only within the reported limits; not a universal proof.
    Bounded {
        /// Explicit limits under which the property was checked.
        bounds: Bounds,
    },
    /// The requested search did not finish, for example at its deadline.
    Incomplete {
        /// Reason exploration stopped.
        reason: String,
        /// Extent of exploration actually completed.
        explored: Bounds,
    },
    /// Backend is unavailable for this run.
    Skipped {
        /// Missing prerequisite.
        reason: String,
    },
    /// Execution failed independently of the property being checked.
    Error {
        /// Redacted backend failure diagnostic.
        reason: String,
    },
    /// No checks are configured for the invariant.
    Uncovered {},
}

/// A runner-supplied, bounded backend invocation. Does not execute by itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckPlan {
    /// Property to check.
    pub invariant: InvariantId,
    /// Project-defined target model.
    pub model: String,
    /// Adapter-local harness handle.
    pub handle: String,
    /// Explicit exploration bounds.
    pub bounds: Bounds,
    /// Deterministic seed, when supported.
    pub seed: Option<u64>,
    /// Positive execution deadline in milliseconds.
    pub timeout_ms: std::num::NonZeroU64,
}

/// Backend output; opaque projected traces avoid depending on verify-scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendReport {
    /// Reporting backend.
    pub backend: BackendId,
    /// Checked invariant.
    pub invariant: InvariantId,
    /// Result and its scope.
    pub verdict: Verdict,
    /// Adapter/tool version actually used, absent when not run.
    pub tool_version: Option<String>,
    /// Projected scenario envelopes for M2 to deserialize and validate.
    pub scenarios: Vec<serde_json::Value>,
}

/// Inverted dependency boundary: the runner registers adapters, never imports them.
pub trait Backend {
    /// Stable, extensible registry key.
    fn id(&self) -> BackendId;
    /// Supported property categories and semantics.
    fn capabilities(&self) -> Capabilities;
    /// Resolve a handle for a model without executing or installing any tool.
    fn has_handle(&self, model: &str, handle: &str) -> bool;
    /// Check execution prerequisites separately from catalog validation.
    fn availability(&self) -> Availability;
    /// Execute an explicitly requested bounded plan.
    fn run(&self, plan: &CheckPlan) -> BackendReport;
}

/// Invalid adapter registration.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    /// Replacing an adapter implicitly would change catalog meaning.
    #[error("backend already registered: {0}")]
    Duplicate(BackendId),
}

/// Borrowed adapter registry owned and populated by the adopting caller.
#[derive(Default)]
pub struct BackendRegistry<'a> {
    backends: BTreeMap<BackendId, &'a dyn Backend>,
}

impl<'a> BackendRegistry<'a> {
    /// Insert without replacing an existing adapter.
    pub fn register(&mut self, backend: &'a dyn Backend) -> Result<(), RegistryError> {
        let id = backend.id();
        match self.backends.entry(id.clone()) {
            std::collections::btree_map::Entry::Occupied(_) => Err(RegistryError::Duplicate(id)),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(backend);
                Ok(())
            }
        }
    }
    /// Resolve a configured key.
    pub fn get(&self, id: &BackendId) -> Option<&'a dyn Backend> {
        self.backends.get(id).copied()
    }
    /// Registered keys in stable order, suitable for actionable errors.
    pub fn ids(&self) -> Vec<BackendId> {
        self.backends.keys().cloned().collect()
    }
}
