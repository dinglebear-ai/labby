//! Invariant identity and classification.
//!
//! An invariant id is the one thing in a catalog that must never change. Every
//! scenario file, every backend binding, and every report row refers to a
//! property by id, so a renamed or reused id silently repoints history at the
//! wrong property. Retirement therefore sets [`Status::Retired`] and never
//! frees the id for reuse.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Maximum length of an invariant id, chosen so ids stay readable in a report
/// column and a scenario filename.
pub const INVARIANT_ID_MAX_LEN: usize = 64;

/// A validated invariant identifier of the form `NAMESPACE-AREA-NNN`, for
/// example `LABBY-REQ-001`.
///
/// The namespace segment is what keeps two projects' catalogs from colliding.
/// It is checked here for shape only; that it matches the *declaring catalog's*
/// namespace is a catalog-level rule, because this type cannot see the catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InvariantId(String);

impl InvariantId {
    /// Parse an id, rejecting anything that does not match `NAMESPACE-AREA-NNN`.
    pub fn parse(raw: &str) -> Result<Self, InvariantIdError> {
        if raw.len() > INVARIANT_ID_MAX_LEN {
            return Err(InvariantIdError::TooLong);
        }
        let mut parts = raw.split('-');
        let namespace = parts.next().unwrap_or_default();
        let area = parts.next().unwrap_or_default();
        let number = parts.next().unwrap_or_default();
        if parts.next().is_some() {
            return Err(InvariantIdError::Malformed);
        }
        if !is_upper_alnum_leading_alpha(namespace) {
            return Err(InvariantIdError::Namespace);
        }
        if area.is_empty() || !area.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(InvariantIdError::Area);
        }
        if number.len() < 3 || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvariantIdError::Number);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The namespace segment: everything before the first `-`.
    pub fn namespace(&self) -> &str {
        self.0.split('-').next().unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_upper_alnum_leading_alpha(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_uppercase() && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

impl fmt::Display for InvariantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for InvariantId {
    type Error = InvariantIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<InvariantId> for String {
    fn from(value: InvariantId) -> Self {
        value.0
    }
}

/// Why an invariant id was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum InvariantIdError {
    #[error("invariant id exceeds {INVARIANT_ID_MAX_LEN} characters")]
    TooLong,
    #[error("invariant id must have exactly three `-` separated segments")]
    Malformed,
    #[error("invariant id namespace must be uppercase alphanumeric and start with a letter")]
    Namespace,
    #[error("invariant id area must be uppercase letters")]
    Area,
    #[error("invariant id number must be at least three digits")]
    Number,
}

/// What kind of property an invariant states.
///
/// This is not decoration: it decides which backends may legitimately claim the
/// invariant. A bounded model checker cannot discharge a liveness property, and
/// the catalog rejects that binding rather than letting a report imply it did.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Something bad never happens. Violated if false at any step.
    Safety,
    /// Something good eventually happens. Needs fairness to discharge.
    Liveness,
    /// A safety property about authority, isolation, or disclosure.
    Security,
    /// An implementation refines a more abstract model.
    Refinement,
}

impl Kind {
    /// Whether a violation is judged over the whole trace rather than the final
    /// state. Safety and security properties are: a violation that is later
    /// repaired still happened, and is usually the interesting counterexample.
    pub const fn violated_at_any_step(self) -> bool {
        matches!(self, Self::Safety | Self::Security)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Safety => "safety",
            Self::Liveness => "liveness",
            Self::Security => "security",
            Self::Refinement => "refinement",
        };
        f.write_str(text)
    }
}

/// How much a violation of this invariant would matter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
}

/// Lifecycle of a catalogued invariant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Checked, and counted in coverage.
    #[default]
    Active,
    /// Written down but not yet claimed by any backend.
    Draft,
    /// No longer checked. The id stays permanently reserved.
    Retired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_id() {
        let id = InvariantId::parse("LABBY-REQ-001").expect("well-formed id");
        assert_eq!(id.namespace(), "LABBY");
        assert_eq!(id.as_str(), "LABBY-REQ-001");
    }

    #[test]
    fn namespace_may_contain_digits_after_the_first_letter() {
        let id =
            InvariantId::parse("X9-PERM-007").expect("digits allowed after the leading letter");
        assert_eq!(id.namespace(), "X9");
    }

    #[test]
    fn rejects_lowercase_and_missing_segments() {
        for bad in ["labby-req-001", "LABBY-REQ", "LABBY", "", "-REQ-001"] {
            assert!(InvariantId::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn rejects_a_short_or_non_numeric_sequence() {
        assert_eq!(
            InvariantId::parse("LABBY-REQ-01"),
            Err(InvariantIdError::Number)
        );
        assert_eq!(
            InvariantId::parse("LABBY-REQ-00A"),
            Err(InvariantIdError::Number)
        );
    }

    #[test]
    fn rejects_extra_segments_rather_than_truncating() {
        // "LABBY-REQ-001-EXTRA" must not quietly parse as LABBY-REQ-001.
        assert_eq!(
            InvariantId::parse("LABBY-REQ-001-EXTRA"),
            Err(InvariantIdError::Malformed)
        );
    }

    #[test]
    fn serde_round_trips_through_a_plain_string() {
        let id = InvariantId::parse("DRIVE-PERM-003").expect("valid");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"DRIVE-PERM-003\"");
        let back: InvariantId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn serde_rejects_an_invalid_id_rather_than_accepting_it() {
        let err = serde_json::from_str::<InvariantId>("\"nope\"");
        assert!(err.is_err(), "deserialization must apply the same rules");
    }

    #[test]
    fn only_safety_and_security_are_judged_over_the_whole_trace() {
        assert!(Kind::Safety.violated_at_any_step());
        assert!(Kind::Security.violated_at_any_step());
        assert!(!Kind::Liveness.violated_at_any_step());
        assert!(!Kind::Refinement.violated_at_any_step());
    }
}
