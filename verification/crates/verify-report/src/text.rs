//! Human-readable rendering of replay results.

use std::fmt::Write as _;

use crate::replay::{Outcome, ReplayReport, Summary};

/// Render one report as a single line plus, on failure, the detail a reader
/// needs to act. Success stays quiet: a run over hundreds of scenarios should
/// not bury its two failures in noise.
pub fn render_text(report: &ReplayReport) -> String {
    let mut out = String::new();
    let verdict = match &report.outcome {
        Outcome::Matched => "ok",
        Outcome::Mismatched { .. } => "MISMATCH",
        Outcome::Promotable => "promotable",
        Outcome::NoTarget { .. } => "NO TARGET",
        Outcome::Malformed { .. } => "MALFORMED",
    };
    let _ = write!(
        out,
        "{verdict:<11} {} [{}]",
        report.scenario, report.invariant
    );

    match &report.outcome {
        Outcome::Mismatched { expected, observed } => {
            let _ = write!(
                out,
                "\n            expected {expected:?}, observed {observed:?}"
            );
            if let Some(index) = report.first_violation {
                let _ = write!(out, "\n            first violated at step {index}");
            }
            if !report.status.gates() {
                let _ = write!(
                    out,
                    "\n            reported only: status is {:?}",
                    report.status
                );
            }
        }
        Outcome::Promotable => {
            let _ = write!(
                out,
                "\n            now matches `expect`; the model has grown the step it was missing"
            );
        }
        Outcome::NoTarget { project, model } => {
            let _ = write!(
                out,
                "\n            no target registered for ({project}, {model})"
            );
        }
        Outcome::Malformed { reason } => {
            let _ = write!(out, "\n            {reason}");
        }
        Outcome::Matched => {}
    }
    out
}

/// Render the aggregate. Names what failed rather than only counting it.
pub fn render_summary(summary: &Summary) -> String {
    let mut out = String::new();
    let _ = write!(out, "{} scenarios", summary.total);
    if summary.matched > 0 {
        let _ = write!(out, ", {} matched", summary.matched);
    }
    if summary.mismatched > 0 {
        let _ = write!(out, ", {} mismatched", summary.mismatched);
    }
    if summary.promotable > 0 {
        let _ = write!(out, ", {} promotable", summary.promotable);
    }
    if summary.no_target > 0 {
        let _ = write!(out, ", {} without a target", summary.no_target);
    }
    if summary.malformed > 0 {
        let _ = write!(out, ", {} malformed", summary.malformed);
    }
    let _ = write!(out, " ({} failing)", summary.failing);
    out
}

trait Gates {
    fn gates(&self) -> bool;
}

impl Gates for verify_scenario::ScenarioStatus {
    fn gates(&self) -> bool {
        matches!(self, Self::Active)
    }
}
