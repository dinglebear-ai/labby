//! A worked example of an adopting project's `verify` binary.
//!
//! The model is deliberately tiny: a request that can be dispatched, cancelled,
//! and completed, with one invariant — a request reaches at most one terminal
//! outcome. It exists to prove the interface end to end without waiting for a
//! real domain model, and to be the thing M3 copies when it wires Labby in.
//!
//! Run it like the real thing:
//!
//! ```text
//! cargo run --example fixture -- replay path/to/scenario.json
//! cargo run --example fixture -- targets
//! ```

use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use verify_core::target::{ScenarioError, ScenarioTarget, StepOutcome};
use verify_core::verdict::Verdict;
use verify_core::{InvariantId, InvariantResult};
use verify_runner::{TargetKey, TargetRegistry, cli};

/// The modeled state: how many terminal outcomes this request has reached.
#[derive(Clone, Debug, Default)]
pub struct RequestState {
    dispatched: bool,
    terminal_outcomes: u32,
}

/// One modeled transition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum RequestStep {
    Dispatch,
    Cancel,
    Complete,
    /// A response arriving after the request was already resolved — the shape
    /// of the bug this fixture exists to demonstrate.
    LateResponse,
}

pub struct RequestModel;

impl ScenarioTarget for RequestModel {
    type State = RequestState;
    type Step = RequestStep;

    fn init(&self, _initial: &serde_json::Value) -> Result<Self::State, ScenarioError> {
        Ok(RequestState::default())
    }

    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError> {
        match step {
            RequestStep::Dispatch => {
                if state.dispatched {
                    // A legal refusal, not a harness failure.
                    return Ok(StepOutcome::rejected("already dispatched"));
                }
                state.dispatched = true;
            }
            RequestStep::Cancel | RequestStep::Complete => {
                if !state.dispatched {
                    return Ok(StepOutcome::rejected("not dispatched"));
                }
                state.terminal_outcomes += 1;
            }
            RequestStep::LateResponse => {
                // Deliberately unguarded: this is the modeled bug.
                state.terminal_outcomes += 1;
            }
        }
        Ok(StepOutcome::Applied)
    }

    fn check(
        &self,
        invariant: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError> {
        if invariant.as_str() != "FIXTURE-REQ-001" {
            return Err(ScenarioError::UnknownInvariant {
                invariant: invariant.clone(),
            });
        }
        let verdict = if state.terminal_outcomes > 1 {
            Verdict::Falsified
        } else {
            Verdict::Verified
        };
        Ok(InvariantResult::new(invariant.clone(), verdict)
            .with_detail(format!("terminal outcomes: {}", state.terminal_outcomes)))
    }
}

/// The registry an adopting project builds. This is the whole integration.
pub fn registry() -> TargetRegistry {
    TargetRegistry::new().with(TargetKey::new("fixture", "request"), Box::new(RequestModel))
}

fn main() -> ExitCode {
    cli::run_from_env(&registry())
}
