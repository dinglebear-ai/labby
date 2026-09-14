//! Verdicts, and the distinctions a report must not blur.
//!
//! Three of these carry most of the toolkit's honesty:
//!
//! - [`Verdict::Bounded`] is not [`Verdict::Verified`]. A proof at `k = 5` or a
//!   model checked over three nodes is not a universal claim, and printing it
//!   as one is how a dashboard comes to imply assurance nobody established.
//! - [`Verdict::Skipped`] is not [`Verdict::Error`]. A missing Java install is
//!   not a failing property.
//! - [`Verdict::Uncovered`] is a legitimate state to ship with, as long as it
//!   is visible. It is reported by id, never as a count.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::invariant::InvariantId;

/// The bound under which a backend established a property.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Bound {
    /// What was limited, for example `depth` or `actors`.
    pub dimension: String,
    /// The limit reached.
    pub limit: u64,
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} <= {}", self.dimension, self.limit)
    }
}

/// The outcome of asking a backend about one invariant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
#[non_exhaustive]
pub enum Verdict {
    /// Discharged without qualification.
    Verified,
    /// A counterexample exists. The scenario, where one could be extracted,
    /// is carried alongside rather than inside the verdict.
    Falsified,
    /// Established only up to the stated bounds. Never collapse this into
    /// `Verified`, however tempting the summary line.
    Bounded { bounds: Vec<Bound> },
    /// The backend was unavailable. Not a property failure.
    Skipped { reason: String },
    /// The backend failed for reasons unrelated to the property.
    Error { reason: String },
    /// No backend claims this invariant.
    Uncovered,
}

impl Verdict {
    /// Whether this verdict represents an established property, bounded or not.
    ///
    /// Deliberately true for `Bounded`: it *is* evidence. The distinction that
    /// matters is in how it is reported, not whether it counts.
    pub const fn is_established(&self) -> bool {
        matches!(self, Self::Verified | Self::Bounded { .. })
    }

    /// Whether this verdict should fail a CI tier that requires the property.
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Falsified | Self::Error { .. })
    }

    /// A short, stable label for report columns and log lines.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Falsified => "falsified",
            Self::Bounded { .. } => "bounded",
            Self::Skipped { .. } => "skipped",
            Self::Error { .. } => "error",
            Self::Uncovered => "uncovered",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bounded { bounds } => {
                let rendered: Vec<String> = bounds.iter().map(ToString::to_string).collect();
                write!(f, "bounded ({})", rendered.join(", "))
            }
            Self::Skipped { reason } | Self::Error { reason } => {
                write!(f, "{} ({reason})", self.label())
            }
            other => f.write_str(other.label()),
        }
    }
}

/// One invariant's verdict, as evaluated against a state or by a backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvariantResult {
    pub invariant: InvariantId,
    pub verdict: Verdict,
    /// Free-text detail for a human reading a failure. Never a secret: this
    /// reaches reports, logs, and committed scenario files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl InvariantResult {
    pub const fn new(invariant: InvariantId, verdict: Verdict) -> Self {
        Self {
            invariant,
            verdict,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> InvariantId {
        InvariantId::parse("LABBY-REQ-001").expect("valid id")
    }

    #[test]
    fn bounded_counts_as_established_but_reports_as_bounded() {
        let verdict = Verdict::Bounded {
            bounds: vec![Bound {
                dimension: "depth".to_owned(),
                limit: 5,
            }],
        };
        assert!(verdict.is_established());
        assert_eq!(verdict.label(), "bounded");
        assert_ne!(verdict.label(), Verdict::Verified.label());
        assert_eq!(verdict.to_string(), "bounded (depth <= 5)");
    }

    #[test]
    fn skipped_is_not_a_failure_and_error_is() {
        let skipped = Verdict::Skipped {
            reason: "alloy not installed".to_owned(),
        };
        let errored = Verdict::Error {
            reason: "solver crashed".to_owned(),
        };
        assert!(!skipped.is_failure());
        assert!(errored.is_failure());
        assert!(!skipped.is_established());
    }

    #[test]
    fn uncovered_is_neither_established_nor_a_failure() {
        // Shipping with uncovered invariants is allowed; hiding them is not.
        assert!(!Verdict::Uncovered.is_established());
        assert!(!Verdict::Uncovered.is_failure());
    }

    #[test]
    fn results_round_trip_and_omit_absent_detail() {
        let result = InvariantResult::new(id(), Verdict::Verified);
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(
            !json.contains("detail"),
            "absent detail must not be emitted"
        );
        let back: InvariantResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, result);
    }

    #[test]
    fn detail_survives_a_round_trip() {
        let result =
            InvariantResult::new(id(), Verdict::Falsified).with_detail("first violated at step 2");
        let json = serde_json::to_string(&result).expect("serialize");
        let back: InvariantResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.detail.as_deref(), Some("first violated at step 2"));
    }
}
