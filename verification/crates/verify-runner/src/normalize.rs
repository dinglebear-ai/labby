use std::{num::NonZeroUsize, time::Instant};

use thiserror::Error;
use verify_scenario::{Expectation, Scenario, Status, ValidatedScenario};

use crate::{ReplayLimits, TargetRegistry, TraceVerdict};

/// Finite work budget for best-effort normalization of trusted target traces.
#[derive(Debug, Clone)]
pub struct NormalizationOptions {
    /// Repeated verdict checks; must be at least three.
    pub determinism_runs: NonZeroUsize,
    /// Total replay attempts, including pre/post determinism checks.
    pub max_replays: NonZeroUsize,
    /// Per-replay cooperative bounds, also used around canonicalization.
    pub replay: ReplayLimits,
}

impl Default for NormalizationOptions {
    fn default() -> Self {
        Self {
            determinism_runs: NonZeroUsize::new(3).expect("positive constant"),
            max_replays: NonZeroUsize::new(256).expect("positive constant"),
            replay: ReplayLimits::default(),
        }
    }
}

/// Normalization output and explicit limitations; insertion is a separate action.
#[derive(Debug, Clone)]
pub struct Normalization {
    /// Validated normalized trace, or original trace on guard failure.
    pub scenario: ValidatedScenario,
    /// Actual replay attempts consumed.
    pub replays: usize,
    /// Shrinking stopped at the finite attempt budget.
    pub budget_exhausted: bool,
    /// A proposed transformation failed validation or changed the verdict.
    pub discarded_transformation: bool,
}

/// Failed normalization, never a passing counterexample or silent corpus write.
#[derive(Debug, Error)]
pub enum NormalizationError {
    /// Requested budget cannot support honest determinism checks.
    #[error("normalization needs 3..=100 determinism runs and 2*N..=4096 replay attempts")]
    Budget,
    /// Target or catalogued invariant could not be resolved.
    #[error("normalization target: {0}")]
    Target(#[from] crate::TargetResolutionError),
    /// Raw trace failed before normalization or was incomplete.
    #[error("normalization requires a completed, error-free raw replay: {0:?}")]
    RawReplay(TraceVerdict),
    /// A transformed envelope was invalid.
    #[error("normalization envelope: {0}")]
    Envelope(#[from] verify_scenario::EnvelopeError),
}

fn reseal(mut raw: Scenario) -> Result<ValidatedScenario, verify_scenario::EnvelopeError> {
    raw.fingerprint = None;
    raw.validate()
}

impl TargetRegistry<'_> {
    /// Canonicalize only through the target's opaque-ID hook, then delta-debug
    /// reproduced counterexamples. Golden/unreproduced traces are never shrunk.
    /// Commutative reordering awaits a real target opting in (M3 or later).
    pub fn normalize(
        &self,
        raw: &ValidatedScenario,
        options: &NormalizationOptions,
    ) -> Result<Normalization, NormalizationError> {
        let n = options.determinism_runs.get();
        let budget = options.max_replays.get();
        if !(3..=100).contains(&n) || budget < 2 * n || budget > 4096 {
            return Err(NormalizationError::Budget);
        }
        let (entry, _) = self.resolve(raw).map_err(NormalizationError::Target)?;
        let baseline = self.replay(raw, &options.replay).verdict;
        if matches!(baseline, TraceVerdict::Error | TraceVerdict::Incomplete) {
            return Err(NormalizationError::RawReplay(baseline));
        }
        let mut result = Normalization {
            scenario: raw.clone(),
            replays: 1,
            budget_exhausted: false,
            discarded_transformation: false,
        };
        for _ in 1..n {
            result.replays += 1;
            if self.replay(raw, &options.replay).verdict != baseline {
                let mut quarantined = raw.scenario().clone();
                quarantined.status = Status::Quarantined;
                result.scenario = reseal(quarantined)?;
                return Ok(result);
            }
        }
        // Keep enough attempts to verify the final candidate N times.
        let started = Instant::now();
        match entry.target.canonicalize(raw) {
            Ok((initial, steps)) if started.elapsed() < options.replay.timeout => {
                let mut candidate = raw.scenario().clone();
                candidate.initial = initial;
                candidate.steps = steps;
                match reseal(candidate) {
                    Ok(candidate) if candidate.fingerprint() == raw.fingerprint() => {}
                    Ok(candidate) if result.replays + n < budget => {
                        result.replays += 1;
                        if self.replay(&candidate, &options.replay).verdict == baseline {
                            result.scenario = candidate;
                        } else {
                            result.discarded_transformation = true;
                        }
                    }
                    Ok(_) => {
                        result.budget_exhausted = true;
                    }
                    Err(_) => {
                        result.discarded_transformation = true;
                    }
                }
            }
            _ => {
                result.discarded_transformation = true;
            }
        }
        let reproduced = raw.scenario().expect == Expectation::InvariantViolated
            && baseline == TraceVerdict::InvariantViolated;
        if reproduced {
            // Deterministic chunk-removal delta debugging, finishing at chunks of
            // one while the budget permits. No minimality claim on exhaustion.
            let mut chunk = result.scenario.scenario().steps.len().div_ceil(2).max(1);
            while !result.scenario.scenario().steps.is_empty() {
                let mut index = 0;
                let mut changed = false;
                while index < result.scenario.scenario().steps.len() {
                    if result.replays + n >= budget {
                        result.budget_exhausted = true;
                        break;
                    }
                    let mut candidate = result.scenario.scenario().clone();
                    let end = (index + chunk).min(candidate.steps.len());
                    candidate.steps.drain(index..end);
                    let candidate = reseal(candidate)?;
                    result.replays += 1;
                    if self.replay(&candidate, &options.replay).verdict == baseline {
                        result.scenario = candidate;
                        changed = true;
                    } else {
                        index += chunk;
                    }
                }
                if result.budget_exhausted {
                    break;
                }
                if chunk == 1 {
                    if !changed {
                        break;
                    }
                } else {
                    chunk = chunk.div_ceil(2);
                }
            }
        }
        // Shrinking may remove an identifier's first appearance. Rename the
        // retained trace again before fingerprinting, with the same verdict guard.
        if result.scenario.fingerprint() != raw.fingerprint() && result.replays + n < budget {
            let started = Instant::now();
            match entry.target.canonicalize(&result.scenario) {
                Ok((initial, steps)) if started.elapsed() < options.replay.timeout => {
                    let mut candidate = result.scenario.scenario().clone();
                    candidate.initial = initial;
                    candidate.steps = steps;
                    match reseal(candidate) {
                        Ok(candidate) => {
                            result.replays += 1;
                            if self.replay(&candidate, &options.replay).verdict == baseline {
                                result.scenario = candidate;
                            } else {
                                result.discarded_transformation = true;
                            }
                        }
                        Err(_) => {
                            result.discarded_transformation = true;
                        }
                    }
                }
                _ => {
                    result.discarded_transformation = true;
                }
            }
        }
        for _ in 0..n {
            result.replays += 1;
            if self.replay(&result.scenario, &options.replay).verdict != baseline {
                let mut quarantined = raw.scenario().clone();
                quarantined.status = Status::Quarantined;
                result.scenario = reseal(quarantined)?;
                result.discarded_transformation = true;
                return Ok(result);
            }
        }
        // Normalization never waives an established regression gate. Importers
        // explicitly classify unreproduced evidence before requesting this API.
        Ok(result)
    }
}
