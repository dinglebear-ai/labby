//! Conversion of retained T0 evidence into the shared report contract.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Write,
    path::Path,
};

use verify_report::{
    ArtifactIdentity, CatalogCoverage, CleanupEvidence, EvidenceLane, EvidenceObservation,
    EvidenceVerdict, ExpectationCounts, Report, Requirement, ScenarioCounts, SourceIdentity,
    StatusCounts, ValidatedReport,
};
use verify_runner::TraceVerdict;
use verify_scenario::{Expectation, Status};

use crate::{T0Report, read_bounded_regular};

pub(crate) fn run(
    args: impl Iterator<Item = OsString>,
    model_checking: bool,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let args: Vec<_> = args.collect();
    if args.len() != 6 {
        let command = if model_checking {
            "report-t1"
        } else {
            "report-t0"
        };
        let _ = writeln!(
            errors,
            "usage: labby-verify {command} <input.json> <revision-file> <dirty-file> <sha256-file> <json|text|markdown|html> <source-name>"
        );
        return 2;
    }
    match convert(&args, model_checking) {
        Ok(text) => match writeln!(output, "{text}") {
            Ok(()) => 0,
            Err(_) => 2,
        },
        Err(error) => {
            let _ = writeln!(errors, "cannot render verification evidence: {error}");
            2
        }
    }
}

fn convert(args: &[OsString], model_checking: bool) -> Result<String, String> {
    let input = read_bounded_regular(Path::new(&args[0]), verify_report::MAX_REPORT_BYTES)?;
    let revision = read_bounded_regular(Path::new(&args[1]), 128)?;
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("source revision must be a full Git SHA".into());
    }
    let dirty = !read_bounded_regular(Path::new(&args[2]), 1_048_576)?
        .trim()
        .is_empty();
    let hash = read_bounded_regular(Path::new(&args[3]), 4096)?;
    let digest = hash.split_whitespace().next().unwrap_or_default();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("binary identity must contain a SHA-256 digest".into());
    }
    let source = SourceIdentity {
        source: args[5].to_str().ok_or("source name is not UTF-8")?.into(),
        revision: Some(revision.into()),
        dirty: Some(dirty),
        unavailable_reason: None,
        dirty_unavailable_reason: None,
    };
    let binary = ArtifactIdentity {
        id: "labby-verify".into(),
        revision: Some(format!("sha256:{digest}")),
        unavailable_reason: None,
    };
    let report = if model_checking {
        from_t1(
            serde_json::from_str(&input).map_err(|error| error.to_string())?,
            source,
            binary,
        )?
    } else {
        from_t0(
            serde_json::from_str(&input).map_err(|error| error.to_string())?,
            source,
            binary,
        )?
    };
    match args[4].to_str() {
        Some("json") => Ok(verify_report::render_json(&report)),
        Some("text") => Ok(verify_report::render_text(&report)),
        Some("markdown") => Ok(verify_report::render_markdown(&report)),
        Some("html") => Ok(verify_report::render_html(&report)),
        _ => Err("unknown report format".into()),
    }
}

fn from_t1(
    t1: crate::checking::T1Report,
    source: SourceIdentity,
    binary: ArtifactIdentity,
) -> Result<ValidatedReport, String> {
    use verify_core::Verdict;
    if t1.schema != 2
        || t1.lane != "model_checking"
        || t1.models
            != labby_model::MODELS
                .iter()
                .map(|model| (*model).to_owned())
                .collect::<Vec<_>>()
        || t1.universal_proof
    {
        return Err("input is not a bounded Labby T1 report".into());
    }
    let mut ids = BTreeSet::new();
    let mut observations = Vec::new();
    for (index, item) in t1.reports.into_iter().enumerate() {
        ids.insert(item.invariant.clone());
        let (verdict, bounds, diagnostic) = match item.verdict {
            Verdict::Bounded { bounds } => (
                EvidenceVerdict::Bounded,
                bounds,
                "Finite Stateright domain only".into(),
            ),
            Verdict::Incomplete { explored, reason } => {
                (EvidenceVerdict::Incomplete, explored, reason)
            }
            Verdict::Falsified { reason } => {
                (EvidenceVerdict::Falsified, Default::default(), reason)
            }
            Verdict::Error { reason } => (EvidenceVerdict::Error, Default::default(), reason),
            Verdict::Skipped { reason } => (EvidenceVerdict::Skipped, Default::default(), reason),
            Verdict::Uncovered {} => (
                EvidenceVerdict::Skipped,
                Default::default(),
                "No configured check".into(),
            ),
            Verdict::Verified {} => return Err("bounded Stateright cannot claim Verified".into()),
        };
        let scenarios = item
            .scenarios
            .iter()
            .map(|value| {
                verify_scenario::Scenario::from_json(&value.to_string())
                    .map(|scenario| scenario.fingerprint())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        observations.push(EvidenceObservation {
            id: format!("model-check-{index}"),
            lane: EvidenceLane::ModelChecking,
            requirement: Requirement::Required,
            verdict,
            invariant_ids: vec![item.invariant],
            scenario_ids: scenarios,
            sources: vec![source.clone()],
            binary: Some(binary.clone()),
            fixtures: vec![ArtifactIdentity {
                id: item.backend.to_string(),
                revision: item.tool_version,
                unavailable_reason: None,
            }],
            seed: None,
            bounds,
            deadline_ms: Some(50_000),
            cleanup: Some(CleanupEvidence {
                completed: true,
                detail: "Checker threads joined before report returned".into(),
            }),
            diagnostic: Some(diagnostic),
        });
    }
    Report {
        schema: 1,
        project: "labby".into(),
        catalog: ArtifactIdentity {
            id: "formal/invariants.toml".into(),
            revision: Some(t1.catalog_fingerprint),
            unavailable_reason: None,
        },
        coverage: CatalogCoverage {
            replay_uncovered_ids: ids.iter().cloned().collect(),
            invariant_ids: ids.into_iter().collect(),
            backend_uncovered_ids: vec![],
            unconfigured_backend_ids: vec![],
        },
        scenarios: ScenarioCounts {
            total: 0,
            by_expectation: ExpectationCounts {
                invariant_holds: 0,
                invariant_violated: 0,
            },
            by_status: StatusCounts {
                active: 0,
                quarantined: 0,
                unreproduced: 0,
            },
        },
        observations,
    }
    .validate()
    .map_err(|error| error.to_string())
}

fn from_t0(
    t0: T0Report,
    source: SourceIdentity,
    binary: ArtifactIdentity,
) -> Result<ValidatedReport, String> {
    if t0.schema != 2
        || t0.lane != "model_replay"
        || t0.universal_proof
        || t0.catalog.models
            != labby_model::MODELS
                .iter()
                .map(|model| (*model).to_owned())
                .collect::<Vec<_>>()
        || t0.catalog.project != "labby"
        || t0.reports.is_empty()
        || t0.golden_coverage.is_empty()
    {
        return Err("input is not a model-only T0 report".into());
    }
    let catalog_ids: BTreeSet<_> = t0
        .golden_coverage
        .iter()
        .map(|item| item.invariant.clone())
        .collect();
    if catalog_ids.len() != t0.golden_coverage.len()
        || t0
            .reports
            .iter()
            .any(|item| !catalog_ids.contains(&item.invariant))
    {
        return Err("T0 catalog identities are duplicated or inconsistent".into());
    }
    let mut derived_failure = t0
        .golden_coverage
        .iter()
        .any(|item| item.active_holds_traces == 0);
    for item in &t0.reports {
        let matches = matches!(
            (item.expectation, item.replay.verdict),
            (Expectation::InvariantHolds, TraceVerdict::InvariantHolds)
                | (
                    Expectation::InvariantViolated,
                    TraceVerdict::InvariantViolated
                )
        );
        if matches != item.replay.matches_expectation
            || item.status != item.replay.status
            || item.replay.gate_failure != (item.status == Status::Active && !matches)
            || item.canonical_fingerprint != item.replay.fingerprint
        {
            return Err("T0 scenario verdict, identity or gate is inconsistent".into());
        }
        derived_failure |= item.replay.gate_failure
            || (item.status == Status::Active && item.normalization.status != "normalized");
    }
    if derived_failure && !t0.gate_failure {
        return Err("T0 gate contradicts retained failing evidence".into());
    }
    for coverage in &t0.golden_coverage {
        let eligible = t0
            .reports
            .iter()
            .filter(|item| {
                item.invariant == coverage.invariant
                    && item.status == Status::Active
                    && item.expectation == Expectation::InvariantHolds
                    && item.replay.matches_expectation
                    && item.normalization.status == "normalized"
            })
            .count();
        if coverage.active_holds_traces > eligible {
            return Err("T0 golden coverage exceeds retained traces".into());
        }
    }
    let unconfigured_backend_ids = t0
        .uncovered_backend_ids
        .iter()
        .map(|id| verify_core::InvariantId::try_from(id.clone()).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut scenario_ids = BTreeMap::<String, Vec<String>>::new();
    for item in &t0.reports {
        scenario_ids
            .entry(item.invariant.clone())
            .or_default()
            .push(item.canonical_fingerprint.clone());
    }
    let mut counts = ScenarioCounts {
        total: t0.reports.len() as u64,
        by_expectation: ExpectationCounts {
            invariant_holds: 0,
            invariant_violated: 0,
        },
        by_status: StatusCounts {
            active: 0,
            quarantined: 0,
            unreproduced: 0,
        },
    };
    let mut observations = Vec::new();
    observations.push(EvidenceObservation {
        id: "t0-source-and-canonical-gate".into(), lane: EvidenceLane::ModelReplay,
        requirement: Requirement::Required,
        verdict: if t0.gate_failure { EvidenceVerdict::Error } else { EvidenceVerdict::Passed },
        invariant_ids: catalog_ids.iter().map(|id| verify_core::InvariantId::try_from(id.clone()).map_err(|error| error.to_string())).collect::<Result<Vec<_>, _>>()?, scenario_ids: scenario_ids.values().flatten().cloned().collect(), sources: vec![source.clone()], binary: Some(binary.clone()),
        fixtures: vec![], seed: None, bounds: Default::default(), deadline_ms: Some(55_000), cleanup: None,
        diagnostic: Some("Original T0 gate, including source replay and semantic coverage; cannot be waived by rendering".into()),
    });
    for (index, item) in t0.reports.into_iter().enumerate() {
        match item.expectation {
            Expectation::InvariantHolds => counts.by_expectation.invariant_holds += 1,
            Expectation::InvariantViolated => counts.by_expectation.invariant_violated += 1,
        }
        match item.status {
            Status::Active => counts.by_status.active += 1,
            Status::Quarantined => counts.by_status.quarantined += 1,
            Status::Unreproduced => counts.by_status.unreproduced += 1,
        }
        let normalization_failed = item.normalization.status != "normalized";
        let verdict = if normalization_failed || item.replay.verdict == TraceVerdict::Error {
            EvidenceVerdict::Error
        } else if item.replay.verdict == TraceVerdict::Incomplete {
            EvidenceVerdict::Incomplete
        } else if item.replay.matches_expectation && !item.replay.gate_failure {
            EvidenceVerdict::Passed
        } else {
            EvidenceVerdict::Falsified
        };
        observations.push(EvidenceObservation {
            id: format!("scenario-{index}"),
            lane: if item.expectation == Expectation::InvariantViolated {
                EvidenceLane::CounterexampleReproduction
            } else { EvidenceLane::ModelReplay },
            requirement: if item.status == Status::Active { Requirement::Required } else { Requirement::Optional },
            verdict,
            invariant_ids: vec![verify_core::InvariantId::try_from(item.invariant).map_err(|error| error.to_string())?],
            scenario_ids: vec![item.canonical_fingerprint],
            sources: vec![source.clone()], binary: Some(binary.clone()),
            fixtures: vec![ArtifactIdentity { id: item.path, revision: Some(item.source_fingerprint), unavailable_reason: None }],
            seed: None,
            bounds: [("completed_observations".into(), serde_json::json!(item.replay.observations.len()))].into(),
            deadline_ms: Some(55_000),
            cleanup: Some(CleanupEvidence { completed: true, detail: "Pure in-process model replay; no external resources created".into() }),
            diagnostic: Some(item.normalization.error.or(item.replay.diagnostic).unwrap_or_else(|| {
                if normalization_failed { "Normalization was guarded or exhausted its finite budget".into() }
                else { "Finite model evidence only; no implementation or actual-host qualification".into() }
            })),
        });
    }
    // Adoption coverage is a separate required observation, not inferred from
    // an empty trace that happens to satisfy a predicate.
    let mut ids = Vec::new();
    for coverage in t0.golden_coverage {
        let id = verify_core::InvariantId::try_from(coverage.invariant)
            .map_err(|error| error.to_string())?;
        ids.push(id.clone());
        observations.push(EvidenceObservation {
            id: format!("golden-coverage-{id}"),
            lane: EvidenceLane::ModelReplay,
            requirement: Requirement::Required,
            verdict: if coverage.active_holds_traces > 0 {
                EvidenceVerdict::Passed
            } else {
                EvidenceVerdict::Error
            },
            scenario_ids: scenario_ids.get(id.as_str()).cloned().unwrap_or_default(),
            invariant_ids: vec![id],
            sources: vec![source.clone()],
            binary: Some(binary.clone()),
            fixtures: vec![],
            seed: None,
            bounds: [(
                "semantic_golden_traces".into(),
                serde_json::json!(coverage.active_holds_traces),
            )]
            .into(),
            deadline_ms: Some(55_000),
            cleanup: None,
            diagnostic: Some(
                if coverage.active_holds_traces > 0 {
                    "Non-vacuous golden coverage"
                } else {
                    "Missing semantic golden coverage"
                }
                .into(),
            ),
        });
    }
    let observed: BTreeSet<_> = observations
        .iter()
        .flat_map(|item| item.invariant_ids.iter())
        .collect();
    let uncovered_ids = ids
        .iter()
        .filter(|id| !observed.contains(id))
        .cloned()
        .collect();
    Report {
        schema: 1,
        project: t0.catalog.project,
        catalog: ArtifactIdentity {
            id: "formal/invariants.toml".into(),
            revision: Some(t0.catalog.fingerprint),
            unavailable_reason: None,
        },
        coverage: CatalogCoverage {
            backend_uncovered_ids: ids.clone(),
            invariant_ids: ids,
            replay_uncovered_ids: uncovered_ids,
            unconfigured_backend_ids,
        },
        scenarios: counts,
        observations,
    }
    .validate()
    .map_err(|error| error.to_string())
}
