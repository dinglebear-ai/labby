use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use verify_core::{Bounds, InvariantId};

/// Maximum diagnostic/reason/provenance text accepted from an execution.
pub const MAX_TEXT_BYTES: usize = 16_384;
/// Maximum encoded report accepted or rendered (16 MiB).
pub const MAX_REPORT_BYTES: usize = 16 * 1_048_576;
/// Maximum observations in one report.
pub const MAX_OBSERVATIONS: usize = 100_000;

/// A distinct source of assurance. Lanes must never be promoted into one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLane {
    /// Formal or executable model exploration.
    ModelChecking,
    /// Deterministic replay against a model only.
    ModelReplay,
    /// Reproduction of a discovered counterexample.
    CounterexampleReproduction,
    /// Controlled model-to-implementation observation comparison.
    Conformance,
    /// A compiled process exercised through a public boundary.
    RealProcess,
    /// A simulator, fake DOM, or socket/browser emulator.
    BrowserEmulation,
    /// A version-pinned interaction with an actual host or provider.
    ActualHost,
}

/// Whether an observation gates the requested qualification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// Absence, incompleteness, falsification, or execution error blocks qualification.
    Required,
    /// Informational evidence which does not gate qualification.
    Optional,
}

/// Honest outcome of one evidence observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceVerdict {
    /// An unbounded property was discharged in its declared semantic domain.
    Verified,
    /// The requested finite execution or comparison completed successfully.
    Passed,
    /// A property or expected observation was contradicted.
    Falsified,
    /// Only the stated finite bounds were explored; never equivalent to verified.
    Bounded,
    /// Execution stopped before reaching a conclusive result.
    Incomplete,
    /// A prerequisite was unavailable.
    Skipped,
    /// The harness or tool failed independently of the checked property.
    Error,
}

/// Caller-supplied source identity; the report performs no repository reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    /// Source name or repository identity.
    pub source: String,
    /// Exact revision when available.
    pub revision: Option<String>,
    /// Dirty state when known.
    pub dirty: Option<bool>,
    /// Why dirty state is unavailable.
    pub dirty_unavailable_reason: Option<String>,
    /// Why revision or dirty state is unavailable.
    pub unavailable_reason: Option<String>,
}

/// Caller-supplied executable or fixture identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    /// Stable artifact name.
    pub id: String,
    /// Version, digest, revision, or content identity.
    pub revision: Option<String>,
    /// Why the identity is unavailable.
    pub unavailable_reason: Option<String>,
}

/// Cleanup evidence from an execution boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupEvidence {
    /// Whether cleanup completed within its deadline.
    pub completed: bool,
    /// Observed cleanup detail, already redacted by the caller.
    pub detail: String,
}

/// One evidence item, retained without collapsing its lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceObservation {
    /// Stable caller-defined observation identifier.
    pub id: String,
    /// Assurance lane.
    pub lane: EvidenceLane,
    /// Qualification requirement.
    pub requirement: Requirement,
    /// Observed result.
    pub verdict: EvidenceVerdict,
    /// Related invariants.
    #[serde(default)]
    pub invariant_ids: Vec<InvariantId>,
    /// Related scenario fingerprints or stable IDs.
    #[serde(default)]
    pub scenario_ids: Vec<String>,
    /// Source identities used by this observation.
    #[serde(default)]
    pub sources: Vec<SourceIdentity>,
    /// Product/model executable identity, if relevant.
    pub binary: Option<ArtifactIdentity>,
    /// Fixture identities.
    #[serde(default)]
    pub fixtures: Vec<ArtifactIdentity>,
    /// Reproduction or exploration seed.
    pub seed: Option<u64>,
    /// Stated exploration/workload bounds.
    #[serde(default)]
    pub bounds: Bounds,
    /// Execution deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Cleanup result, when an execution boundary needs cleanup.
    pub cleanup: Option<CleanupEvidence>,
    /// Required reason for skipped, incomplete, or error; optional redacted detail otherwise.
    pub diagnostic: Option<String>,
}

/// Catalog coverage accounting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCoverage {
    /// Every invariant in the selected catalog.
    pub invariant_ids: Vec<InvariantId>,
    /// IDs without model-replay evidence.
    pub replay_uncovered_ids: Vec<InvariantId>,
    /// IDs with no configured model-checking backend binding.
    pub unconfigured_backend_ids: Vec<InvariantId>,
    /// IDs without model-checking backend evidence.
    pub backend_uncovered_ids: Vec<InvariantId>,
}

/// Scenario corpus accounting, kept independent from execution verdicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioCounts {
    /// Total selected scenarios.
    pub total: u64,
    /// Counts by expectation.
    pub by_expectation: ExpectationCounts,
    /// Counts by reproduction status.
    pub by_status: StatusCounts,
}

/// Exhaustive scenario expectation counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectationCounts {
    /// Scenarios expecting the invariant to hold.
    pub invariant_holds: u64,
    /// Scenarios expecting a concrete violation.
    pub invariant_violated: u64,
}

/// Exhaustive scenario lifecycle counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusCounts {
    /// Gating scenarios.
    pub active: u64,
    /// Nondeterministic, non-gating scenarios.
    pub quarantined: u64,
    /// Scenarios awaiting reproduction.
    pub unreproduced: u64,
}

/// Stable JSON report contract. Validate before rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// Contract version, currently one.
    pub schema: u32,
    /// Adopting project.
    pub project: String,
    /// Catalog identity supplied by the caller.
    pub catalog: ArtifactIdentity,
    /// Catalog coverage.
    pub coverage: CatalogCoverage,
    /// Scenario accounting.
    pub scenarios: ScenarioCounts,
    /// Evidence observations.
    pub observations: Vec<EvidenceObservation>,
}

/// Qualification derived from required evidence only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualification {
    /// Every required observation passed or verified.
    Passed,
    /// At least one required observation did not pass.
    Blocked,
}

/// Validated report accepted by every renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedReport(Report);

/// Contract validation failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReportError {
    /// JSON does not match the stable contract.
    #[error("invalid report JSON: {0}")]
    Json(String),
    /// A semantic contract was violated.
    #[error("invalid report: {0}")]
    Invalid(&'static str),
}

impl Report {
    /// Parse the strict wire shape and validate semantic accounting.
    pub fn from_json(input: &str) -> Result<ValidatedReport, ReportError> {
        if input.len() > MAX_REPORT_BYTES {
            return Err(ReportError::Invalid("report exceeds encoded size limit"));
        }
        serde_json::from_str::<Self>(input)
            .map_err(|error| ReportError::Json(error.to_string()))?
            .validate()
    }

    /// Validate constructed reports without consulting ambient state.
    pub fn validate(self) -> Result<ValidatedReport, ReportError> {
        if self.schema != 1 {
            return Err(ReportError::Invalid("schema must be 1"));
        }
        check_text(&self.project)?;
        check_artifact(&self.catalog)?;
        if self.observations.len() > MAX_OBSERVATIONS {
            return Err(ReportError::Invalid("too many observations"));
        }
        let catalog: BTreeSet<_> = self.coverage.invariant_ids.iter().collect();
        if catalog.is_empty() || catalog.len() != self.coverage.invariant_ids.len() {
            return Err(ReportError::Invalid(
                "catalog must contain unique invariant ids",
            ));
        }
        let replay_uncovered: BTreeSet<_> = self.coverage.replay_uncovered_ids.iter().collect();
        let unconfigured_backend: BTreeSet<_> =
            self.coverage.unconfigured_backend_ids.iter().collect();
        let backend_uncovered: BTreeSet<_> = self.coverage.backend_uncovered_ids.iter().collect();
        check_uncovered(
            &catalog,
            &replay_uncovered,
            self.coverage.replay_uncovered_ids.len(),
        )?;
        check_uncovered(
            &catalog,
            &unconfigured_backend,
            self.coverage.unconfigured_backend_ids.len(),
        )?;
        check_uncovered(
            &catalog,
            &backend_uncovered,
            self.coverage.backend_uncovered_ids.len(),
        )?;
        let observed: BTreeSet<_> = self
            .observations
            .iter()
            .flat_map(|o| o.invariant_ids.iter())
            .collect();
        if !observed.is_subset(&catalog) {
            return Err(ReportError::Invalid(
                "observation references unknown invariant",
            ));
        }
        let replay_observed: BTreeSet<_> = self
            .observations
            .iter()
            .filter(|observation| {
                matches!(
                    observation.lane,
                    EvidenceLane::ModelReplay | EvidenceLane::CounterexampleReproduction
                )
            })
            .flat_map(|observation| observation.invariant_ids.iter())
            .collect();
        let backend_observed = observed_in_lane(&self.observations, EvidenceLane::ModelChecking);
        if catalog
            .difference(&replay_observed)
            .copied()
            .collect::<BTreeSet<_>>()
            != replay_uncovered
        {
            return Err(ReportError::Invalid(
                "replay uncovered invariant ids do not match observations",
            ));
        }
        if catalog
            .difference(&backend_observed)
            .copied()
            .collect::<BTreeSet<_>>()
            != backend_uncovered
        {
            return Err(ReportError::Invalid(
                "backend uncovered invariant ids do not match observations",
            ));
        }
        if self
            .scenarios
            .by_expectation
            .invariant_holds
            .checked_add(self.scenarios.by_expectation.invariant_violated)
            != Some(self.scenarios.total)
        {
            return Err(ReportError::Invalid(
                "scenario expectation counts do not equal total",
            ));
        }
        if self
            .scenarios
            .by_status
            .active
            .checked_add(self.scenarios.by_status.quarantined)
            .and_then(|count| count.checked_add(self.scenarios.by_status.unreproduced))
            != Some(self.scenarios.total)
        {
            return Err(ReportError::Invalid(
                "scenario status counts do not equal total",
            ));
        }
        let mut observation_ids = BTreeSet::new();
        for observation in &self.observations {
            check_observation(observation)?;
            if !observation_ids.insert(&observation.id) {
                return Err(ReportError::Invalid("duplicate observation id"));
            }
        }
        if serde_json::to_vec(&self)
            .map_err(|error| ReportError::Json(error.to_string()))?
            .len()
            > MAX_REPORT_BYTES
        {
            return Err(ReportError::Invalid("report exceeds encoded size limit"));
        }
        Ok(ValidatedReport(self))
    }
}

impl ValidatedReport {
    /// Borrow the stable JSON contract.
    pub fn report(&self) -> &Report {
        &self.0
    }
    /// Derive qualification without allowing a caller-supplied green status.
    pub fn qualification(&self) -> Qualification {
        let required: Vec<_> = self
            .0
            .observations
            .iter()
            .filter(|o| o.requirement == Requirement::Required)
            .collect();
        if !required.is_empty()
            && required.iter().all(|o| {
                matches!(
                    o.verdict,
                    EvidenceVerdict::Verified | EvidenceVerdict::Passed
                ) && o.cleanup.as_ref().is_none_or(|cleanup| cleanup.completed)
                    && metadata_available(o)
            })
        {
            Qualification::Passed
        } else {
            Qualification::Blocked
        }
    }
}

fn check_observation(value: &EvidenceObservation) -> Result<(), ReportError> {
    check_text(&value.id)?;
    for id in &value.scenario_ids {
        check_text(id)?;
    }
    for source in &value.sources {
        check_text(&source.source)?;
        check_available(
            source.revision.as_deref(),
            source.unavailable_reason.as_deref(),
        )?;
        check_known(
            source.dirty.is_some(),
            source.dirty_unavailable_reason.as_deref(),
            "dirty state requires a value or unavailable reason",
        )?;
        if let Some(reason) = &source.unavailable_reason {
            check_text(reason)?;
        }
        if let Some(reason) = &source.dirty_unavailable_reason {
            check_text(reason)?;
        }
    }
    if let Some(binary) = &value.binary {
        check_artifact(binary)?;
    }
    for fixture in &value.fixtures {
        check_artifact(fixture)?;
    }
    if matches!(
        value.verdict,
        EvidenceVerdict::Bounded | EvidenceVerdict::Incomplete
    ) && value.bounds.is_empty()
    {
        return Err(ReportError::Invalid(
            "bounded or incomplete observation requires explored bounds",
        ));
    }
    if matches!(
        value.verdict,
        EvidenceVerdict::Verified | EvidenceVerdict::Bounded
    ) && value.lane != EvidenceLane::ModelChecking
    {
        return Err(ReportError::Invalid(
            "verified and bounded verdicts are restricted to model checking",
        ));
    }
    if matches!(
        value.verdict,
        EvidenceVerdict::Incomplete | EvidenceVerdict::Skipped | EvidenceVerdict::Error
    ) && value
        .diagnostic
        .as_ref()
        .is_none_or(|v| v.trim().is_empty())
    {
        return Err(ReportError::Invalid(
            "incomplete, skipped, or error observation requires a reason",
        ));
    }
    if matches!(
        value.lane,
        EvidenceLane::RealProcess | EvidenceLane::Conformance | EvidenceLane::ActualHost
    ) && value.verdict != EvidenceVerdict::Skipped
        && value.binary.is_none()
    {
        return Err(ReportError::Invalid(
            "implementation evidence requires binary identity or honest unavailability",
        ));
    }
    let implementation_execution = matches!(
        value.lane,
        EvidenceLane::Conformance
            | EvidenceLane::RealProcess
            | EvidenceLane::BrowserEmulation
            | EvidenceLane::ActualHost
    ) && value.verdict != EvidenceVerdict::Skipped;
    if implementation_execution
        && (value.sources.is_empty()
            || value.deadline_ms.is_none_or(|deadline| deadline == 0)
            || value.cleanup.is_none())
    {
        return Err(ReportError::Invalid(
            "implementation execution requires source, nonzero deadline, and cleanup evidence",
        ));
    }
    if value.deadline_ms == Some(0) {
        return Err(ReportError::Invalid("deadline must be nonzero"));
    }
    if matches!(
        value.verdict,
        EvidenceVerdict::Verified | EvidenceVerdict::Passed
    ) {
        if value.invariant_ids.is_empty() {
            return Err(ReportError::Invalid(
                "passing evidence requires an invariant id",
            ));
        }
        if matches!(
            value.lane,
            EvidenceLane::ModelReplay | EvidenceLane::CounterexampleReproduction
        ) && value.scenario_ids.is_empty()
        {
            return Err(ReportError::Invalid(
                "replay evidence requires a scenario id",
            ));
        }
        if value.sources.is_empty() || value.deadline_ms.is_none() {
            return Err(ReportError::Invalid(
                "passing evidence requires source and deadline provenance",
            ));
        }
        if matches!(
            value.lane,
            EvidenceLane::ModelChecking
                | EvidenceLane::CounterexampleReproduction
                | EvidenceLane::Conformance
                | EvidenceLane::RealProcess
                | EvidenceLane::ActualHost
        ) && value.binary.is_none()
        {
            return Err(ReportError::Invalid(
                "passing evidence requires binary identity",
            ));
        }
    }
    if let Some(diagnostic) = &value.diagnostic {
        check_text(diagnostic)?;
    }
    if let Some(cleanup) = &value.cleanup {
        check_text(&cleanup.detail)?;
    }
    Ok(())
}

fn check_artifact(value: &ArtifactIdentity) -> Result<(), ReportError> {
    check_text(&value.id)?;
    check_available(
        value.revision.as_deref(),
        value.unavailable_reason.as_deref(),
    )?;
    if let Some(reason) = &value.unavailable_reason {
        check_text(reason)?;
    }
    Ok(())
}

fn check_available(identity: Option<&str>, reason: Option<&str>) -> Result<(), ReportError> {
    if identity.is_some() == reason.is_some() {
        return Err(ReportError::Invalid(
            "identity requires exactly one of revision or unavailable reason",
        ));
    }
    if let Some(identity) = identity {
        check_text(identity)?;
    }
    Ok(())
}

fn check_known(known: bool, reason: Option<&str>, error: &'static str) -> Result<(), ReportError> {
    if known == reason.is_some() {
        return Err(ReportError::Invalid(error));
    }
    Ok(())
}

fn check_text(value: &str) -> Result<(), ReportError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        Err(ReportError::Invalid("text is empty or exceeds limit"))
    } else if value.chars().any(|character| {
        (character.is_control() && !matches!(character, '\n' | '\t')) || character == '\u{7f}'
    }) {
        Err(ReportError::Invalid(
            "text contains a disallowed control character",
        ))
    } else {
        Ok(())
    }
}

fn check_uncovered<'a>(
    catalog: &BTreeSet<&'a InvariantId>,
    uncovered: &BTreeSet<&'a InvariantId>,
    supplied_len: usize,
) -> Result<(), ReportError> {
    if uncovered.len() != supplied_len || !uncovered.is_subset(catalog) {
        return Err(ReportError::Invalid("invalid uncovered invariant ids"));
    }
    Ok(())
}

fn observed_in_lane(
    observations: &[EvidenceObservation],
    lane: EvidenceLane,
) -> BTreeSet<&InvariantId> {
    observations
        .iter()
        .filter(|observation| observation.lane == lane)
        .flat_map(|observation| observation.invariant_ids.iter())
        .collect()
}

fn metadata_available(observation: &EvidenceObservation) -> bool {
    observation
        .sources
        .iter()
        .all(|source| source.revision.is_some() && source.dirty.is_some())
        && observation
            .binary
            .as_ref()
            .is_none_or(|binary| binary.revision.is_some())
        && observation
            .fixtures
            .iter()
            .all(|fixture| fixture.revision.is_some())
}
