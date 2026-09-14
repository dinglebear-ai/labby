//! The backend contract, and the capability vocabulary that gates it.
//!
//! This trait lives in `verify-core` rather than in the runner so that
//! `verify-runner` depends on no backend crate. An adopting project registers
//! the implementations it wants; that inversion is what lets a project use
//! Stateright without installing a Java toolchain.
//!
//! Nothing in this crate implements [`Backend`]. It is defined here because
//! catalog validation needs to ask a backend what it can honestly claim.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::invariant::Kind;

/// Identifier of a backend, matching the key used in a catalog's `checks`
/// table: `stateright`, `kani`, `loom`, `alloy`, `tla`, and so on.
///
/// The set is deliberately open. Enumerating backends in a schema would make
/// every new backend a schema bump, which the extraction gates cannot survive.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BackendId(String);

impl BackendId {
    pub fn parse(raw: &str) -> Result<Self, BackendIdError> {
        let mut bytes = raw.bytes();
        let Some(first) = bytes.next() else {
            return Err(BackendIdError::Empty);
        };
        if !first.is_ascii_lowercase() {
            return Err(BackendIdError::Shape);
        }
        if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(BackendIdError::Shape);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for BackendId {
    type Error = BackendIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<BackendId> for String {
    fn from(value: BackendId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BackendIdError {
    #[error("backend id must not be empty")]
    Empty,
    #[error("backend id must be lowercase alphanumeric with underscores, starting with a letter")]
    Shape,
}

/// What a backend can honestly establish.
///
/// A backend that cannot express fairness declares no liveness capability, and
/// catalog validation then refuses to let a liveness invariant bind to it. The
/// alternative — accepting the binding and reporting green — is the failure
/// this whole layer exists to prevent.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Kinds of property this backend may claim.
    pub kinds: BTreeSet<Kind>,
    /// Whether results are inherently bounded (a depth, a node count, a `k`).
    /// A bounded backend's successes are reported as `Bounded`, never
    /// `Verified`.
    pub bounded: bool,
    /// Whether the backend can project a counterexample into a scenario.
    /// A backend that cannot says so, and reports `Falsified` with a diagnostic
    /// instead of fabricating a trace.
    pub produces_counterexamples: bool,
}

impl Capabilities {
    pub fn claims(&self, kind: Kind) -> bool {
        self.kinds.contains(&kind)
    }
}

/// Whether a backend can run here and now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "availability")]
pub enum Availability {
    Ready,
    /// The backend's toolchain is absent. This yields `Skipped` verdicts rather
    /// than a build failure, except in a CI tier that explicitly requires it.
    Missing {
        reason: String,
    },
}

/// A verification backend.
///
/// Implementors live in their own crates (`verify-stateright`, `verify-kani`,
/// and so on) so that a consumer pays for neither a Java toolchain nor a CBMC
/// install unless it opts in.
pub trait Backend {
    fn id(&self) -> BackendId;
    fn capabilities(&self) -> Capabilities;
    fn availability(&self) -> Availability;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_ids_accept_the_documented_shape() {
        for good in ["stateright", "kani", "tla", "verify_x2"] {
            assert!(BackendId::parse(good).is_ok(), "{good:?} should parse");
        }
    }

    #[test]
    fn backend_ids_reject_uppercase_dashes_and_empty() {
        for bad in ["Stateright", "state-right", "", "9kani"] {
            assert!(BackendId::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn capabilities_report_only_declared_kinds() {
        let caps = Capabilities {
            kinds: std::iter::once(Kind::Safety).collect(),
            bounded: true,
            produces_counterexamples: true,
        };
        assert!(caps.claims(Kind::Safety));
        // A bounded checker with no fairness must not claim liveness.
        assert!(!caps.claims(Kind::Liveness));
    }

    #[test]
    fn default_capabilities_claim_nothing() {
        // Defaulting to "claims everything" would make the gate useless for a
        // backend whose author forgot to fill this in.
        let caps = Capabilities::default();
        for kind in [
            Kind::Safety,
            Kind::Liveness,
            Kind::Security,
            Kind::Refinement,
        ] {
            assert!(!caps.claims(kind));
        }
    }
}
