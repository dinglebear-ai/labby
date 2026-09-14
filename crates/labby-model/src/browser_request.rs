use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use verify_core::{InvariantId, InvariantResult, ScenarioError, ScenarioTarget, StepOutcome};

/// Whether an admitted browser request may already have reached page code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPhase {
    /// Published locally but not delivered to the browser.
    Admitted,
    /// Delivered to the browser; cancellation cannot undo possible effects.
    Dispatched,
}

/// The first and only authoritative terminal observation for a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Success,
    Error,
    CancelledBeforeDispatch,
    CancelledAfterDispatch,
    TimedOut,
    BrowserOffline,
    StaleDocument,
}

/// An active request and its exact connection-generation owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveRequest {
    pub generation: String,
    pub phase: RequestPhase,
}

/// Fully observable abstract state used by replay and conformance adapters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserRequestState {
    pub current_generation: Option<String>,
    pub active: BTreeMap<String, ActiveRequest>,
    pub terminal: BTreeMap<String, TerminalOutcome>,
    /// Immutable snapshot of the first terminal outcome for overwrite checks.
    pub first_terminal: BTreeMap<String, TerminalOutcome>,
    pub terminal_writes: BTreeMap<String, u8>,
    /// Generation that owned each admitted request, retained after cleanup.
    pub request_generations: BTreeMap<String, String>,
    /// Generation accepted as the authority for success/error completion.
    pub completion_generations: BTreeMap<String, String>,
    /// Whether the completing generation was current at the completion boundary.
    pub completion_was_current: BTreeMap<String, bool>,
    /// Requests that crossed dispatch and may have caused page effects.
    pub dispatched: BTreeSet<String>,
    /// Every connection generation used; production generations are fresh UUIDs.
    pub seen_generations: BTreeSet<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InitialState {
    #[serde(default)]
    current_generation: Option<String>,
}

/// Controlled lifecycle events. Payloads are identifiers, never request data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Connect {
        generation: String,
    },
    ReplaceConnection {
        generation: String,
    },
    Admit {
        request: String,
    },
    Dispatch {
        request: String,
    },
    /// Infrastructure rejected the request before it could reach page code.
    FailBeforeDispatch {
        request: String,
    },
    CompleteSuccess {
        request: String,
        generation: String,
    },
    CompleteError {
        request: String,
        generation: String,
    },
    Cancel {
        request: String,
    },
    Timeout {
        request: String,
    },
    Disconnect {
        generation: String,
    },
    InvalidateDocument {
        request: String,
    },
}

/// Browser Bridge page-call lifecycle model.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrowserRequestModel;

impl BrowserRequestModel {
    fn rejected(reason: impl Into<String>) -> StepOutcome {
        StepOutcome::Rejected {
            reason: reason.into(),
        }
    }

    fn terminalize(
        state: &mut BrowserRequestState,
        request: &str,
        outcome: TerminalOutcome,
    ) -> StepOutcome {
        let Some(_) = state.active.remove(request) else {
            return Self::rejected("request is not active");
        };
        if state.terminal.contains_key(request) {
            return Self::rejected("request already has a terminal outcome");
        }
        state.terminal.insert(request.to_owned(), outcome);
        state
            .first_terminal
            .insert(request.to_owned(), state.terminal[request].clone());
        *state.terminal_writes.entry(request.to_owned()).or_default() += 1;
        StepOutcome::Applied {}
    }

    fn finish_generation(state: &mut BrowserRequestState, generation: &str) {
        let owned: Vec<_> = state
            .active
            .iter()
            .filter(|(_, request)| request.generation == generation)
            .map(|(id, _)| id.clone())
            .collect();
        for request in owned {
            drop(Self::terminalize(
                state,
                &request,
                TerminalOutcome::BrowserOffline,
            ));
        }
    }

    fn complete(
        state: &mut BrowserRequestState,
        request: &str,
        generation: &str,
        outcome: TerminalOutcome,
    ) -> StepOutcome {
        if state.current_generation.as_deref() != Some(generation) {
            return Self::rejected("completion came from a stale connection generation");
        }
        let Some(active) = state.active.get(request) else {
            return Self::rejected("late response has no active request");
        };
        if active.generation != generation {
            return Self::rejected("completion does not own the request generation");
        }
        if active.phase != RequestPhase::Dispatched {
            return Self::rejected("request was not dispatched");
        }
        state
            .completion_generations
            .insert(request.to_owned(), generation.to_owned());
        state
            .completion_was_current
            .insert(request.to_owned(), true);
        Self::terminalize(state, request, outcome)
    }
}

impl ScenarioTarget for BrowserRequestModel {
    type State = BrowserRequestState;
    type Step = Step;

    fn init(&self, initial: &Value) -> Result<Self::State, ScenarioError> {
        let decoded: InitialState = serde_json::from_value(initial.clone())
            .map_err(|error| ScenarioError::InvalidInitial(error.to_string()))?;
        if decoded.current_generation.as_deref() == Some("") {
            return Err(ScenarioError::InvalidInitial(
                "current_generation must not be empty".into(),
            ));
        }
        let mut state = BrowserRequestState {
            current_generation: decoded.current_generation,
            ..BrowserRequestState::default()
        };
        if let Some(generation) = &state.current_generation {
            state.seen_generations.insert(generation.clone());
        }
        Ok(state)
    }

    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError> {
        let outcome = match step {
            Step::Connect { generation } => {
                if generation.is_empty()
                    || state.current_generation.is_some()
                    || state.seen_generations.contains(generation)
                {
                    Self::rejected("connection generation is empty, active, or reused")
                } else {
                    state.current_generation = Some(generation.clone());
                    state.seen_generations.insert(generation.clone());
                    StepOutcome::Applied {}
                }
            }
            Step::ReplaceConnection { generation } => {
                if generation.is_empty() || state.seen_generations.contains(generation) {
                    Self::rejected("connection generation is empty or reused")
                } else {
                    if let Some(previous) = state.current_generation.replace(generation.clone()) {
                        Self::finish_generation(state, &previous);
                    }
                    state.seen_generations.insert(generation.clone());
                    StepOutcome::Applied {}
                }
            }
            Step::Admit { request } => {
                let Some(generation) = state.current_generation.clone() else {
                    return Ok(Self::rejected("browser is offline"));
                };
                if request.is_empty()
                    || state.active.contains_key(request)
                    || state.terminal.contains_key(request)
                {
                    Self::rejected("request id is empty or already used")
                } else {
                    state.active.insert(
                        request.clone(),
                        ActiveRequest {
                            generation: generation.clone(),
                            phase: RequestPhase::Admitted,
                        },
                    );
                    state
                        .request_generations
                        .insert(request.clone(), generation);
                    StepOutcome::Applied {}
                }
            }
            Step::Dispatch { request } => match state.active.get_mut(request) {
                Some(active) if active.phase == RequestPhase::Admitted => {
                    active.phase = RequestPhase::Dispatched;
                    state.dispatched.insert(request.clone());
                    StepOutcome::Applied {}
                }
                _ => Self::rejected("request is not awaiting dispatch"),
            },
            Step::FailBeforeDispatch { request } => {
                if !state
                    .active
                    .get(request)
                    .is_some_and(|active| active.phase == RequestPhase::Admitted)
                {
                    Self::rejected("request is not awaiting dispatch")
                } else {
                    Self::terminalize(state, request, TerminalOutcome::Error)
                }
            }
            Step::CompleteSuccess {
                request,
                generation,
            } => Self::complete(state, request, generation, TerminalOutcome::Success),
            Step::CompleteError {
                request,
                generation,
            } => Self::complete(state, request, generation, TerminalOutcome::Error),
            Step::Cancel { request } => {
                let Some(active) = state.active.get(request) else {
                    return Ok(Self::rejected("request is not active"));
                };
                let terminal = match active.phase {
                    RequestPhase::Admitted => TerminalOutcome::CancelledBeforeDispatch,
                    RequestPhase::Dispatched => TerminalOutcome::CancelledAfterDispatch,
                };
                Self::terminalize(state, request, terminal)
            }
            Step::Timeout { request } => {
                if !state
                    .active
                    .get(request)
                    .is_some_and(|active| active.phase == RequestPhase::Dispatched)
                {
                    Self::rejected("request timeout starts only after dispatch")
                } else {
                    Self::terminalize(state, request, TerminalOutcome::TimedOut)
                }
            }
            Step::Disconnect { generation } => {
                if state.current_generation.as_deref() != Some(generation) {
                    Self::rejected("disconnect came from a stale connection generation")
                } else {
                    Self::finish_generation(state, generation);
                    state.current_generation = None;
                    StepOutcome::Applied {}
                }
            }
            Step::InvalidateDocument { request } => {
                Self::terminalize(state, request, TerminalOutcome::StaleDocument)
            }
        };
        Ok(outcome)
    }

    fn check(
        &self,
        id: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError> {
        let violation =
            match id.as_str() {
                "LABBY-REQ-001" => state
                    .terminal_writes
                    .iter()
                    .find(|(_, writes)| **writes > 1)
                    .map(|(request, _)| format!("request {request} has multiple terminal writes")),
                "LABBY-REQ-002" => state
                    .active
                    .keys()
                    .find(|request| state.terminal.contains_key(*request))
                    .map(|request| format!("terminal request {request} remains active")),
                "LABBY-REQ-003" => state.terminal.iter().find_map(|(request, terminal)| {
                    (state.first_terminal.get(request) != Some(terminal))
                        .then(|| format!("terminal outcome for {request} was overwritten"))
                }),
                "LABBY-REQ-004" => state.completion_generations.iter().find_map(
                    |(request, completion_generation)| {
                        (state.request_generations.get(request) != Some(completion_generation)
                            || state.completion_was_current.get(request) != Some(&true))
                        .then(|| {
                            format!(
                                "completion for {request} came from a stale or non-owner generation"
                            )
                        })
                    },
                ),
                "LABBY-REQ-005" => state.terminal.iter().find_map(|(request, terminal)| {
                    (matches!(terminal, TerminalOutcome::CancelledBeforeDispatch)
                        && state.dispatched.contains(request))
                    .then(|| format!("pre-dispatch cancellation marked {request} as dispatched"))
                }),
                _ => return Err(ScenarioError::UnknownInvariant(id.clone())),
            };
        Ok(violation.map_or(InvariantResult::Holds {}, |reason| {
            InvariantResult::Violated { reason }
        }))
    }

    fn canonicalize(
        &self,
        initial: &Value,
        steps: &[Self::Step],
    ) -> Result<(Value, Vec<Self::Step>), ScenarioError> {
        drop(self.init(initial)?);
        let mut initial: InitialState = serde_json::from_value(initial.clone())
            .map_err(|error| ScenarioError::InvalidInitial(error.to_string()))?;
        let mut generations = BTreeMap::<String, String>::new();
        let mut requests = BTreeMap::<String, String>::new();
        let mut next_generation = 0usize;
        let mut next_request = 0usize;
        let mut rename_generation = |value: &mut String| {
            if value.is_empty() {
                return;
            }
            let replacement = generations
                .entry(value.clone())
                .or_insert_with(|| {
                    let id = format!("generation_{next_generation}");
                    next_generation += 1;
                    id
                })
                .clone();
            *value = replacement;
        };
        if let Some(generation) = &mut initial.current_generation {
            rename_generation(generation);
        }
        let mut canonical = steps.to_vec();
        for step in &mut canonical {
            match step {
                Step::Connect { generation }
                | Step::ReplaceConnection { generation }
                | Step::Disconnect { generation } => rename_generation(generation),
                Step::CompleteSuccess {
                    request,
                    generation,
                }
                | Step::CompleteError {
                    request,
                    generation,
                } => {
                    rename_generation(generation);
                    rename_request(request, &mut requests, &mut next_request);
                }
                Step::Admit { request }
                | Step::Dispatch { request }
                | Step::FailBeforeDispatch { request }
                | Step::Cancel { request }
                | Step::Timeout { request }
                | Step::InvalidateDocument { request } => {
                    rename_request(request, &mut requests, &mut next_request);
                }
            }
        }
        let initial = serde_json::to_value(initial)
            .map_err(|error| ScenarioError::Harness(error.to_string()))?;
        Ok((initial, canonical))
    }
}

fn rename_request(value: &mut String, names: &mut BTreeMap<String, String>, next: &mut usize) {
    if value.is_empty() {
        return;
    }
    let replacement = names
        .entry(value.clone())
        .or_insert_with(|| {
            let id = format!("request_{next}");
            *next += 1;
            id
        })
        .clone();
    *value = replacement;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invariant(value: &str) -> InvariantId {
        InvariantId::try_from(value.to_owned()).unwrap()
    }

    #[test]
    fn late_response_after_cancel_is_rejected_without_overwrite() {
        let model = BrowserRequestModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [
            Step::Connect {
                generation: "g7".into(),
            },
            Step::Admit {
                request: "r9".into(),
            },
            Step::Dispatch {
                request: "r9".into(),
            },
            Step::Cancel {
                request: "r9".into(),
            },
        ] {
            assert_eq!(
                model.apply(&mut state, &step).unwrap(),
                StepOutcome::Applied {}
            );
        }
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::CompleteSuccess {
                        request: "r9".into(),
                        generation: "g7".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
        assert_eq!(
            state.terminal["r9"],
            TerminalOutcome::CancelledAfterDispatch
        );
        assert_eq!(
            model.check(&invariant("LABBY-REQ-001"), &state).unwrap(),
            InvariantResult::Holds {}
        );
    }

    #[test]
    fn stale_generation_cannot_complete_or_disconnect_current_request() {
        let model = BrowserRequestModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [
            Step::Connect {
                generation: "old".into(),
            },
            Step::ReplaceConnection {
                generation: "new".into(),
            },
            Step::Admit {
                request: "call".into(),
            },
            Step::Dispatch {
                request: "call".into(),
            },
        ] {
            drop(model.apply(&mut state, &step).unwrap());
        }
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::Disconnect {
                        generation: "old".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::CompleteSuccess {
                        request: "call".into(),
                        generation: "old".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
        assert_eq!(
            model
                .apply(
                    &mut state,
                    &Step::CompleteSuccess {
                        request: "call".into(),
                        generation: "new".into()
                    }
                )
                .unwrap(),
            StepOutcome::Applied {}
        );
    }

    #[test]
    fn canonicalization_renames_ids_consistently_without_changing_length() {
        let model = BrowserRequestModel;
        let steps = vec![
            Step::Connect {
                generation: "socket-z".into(),
            },
            Step::Admit {
                request: "uuid-q".into(),
            },
            Step::CompleteError {
                request: "uuid-q".into(),
                generation: "socket-z".into(),
            },
        ];
        let (_, normalized) = model.canonicalize(&serde_json::json!({}), &steps).unwrap();
        assert_eq!(normalized.len(), steps.len());
        assert_eq!(
            normalized[0],
            Step::Connect {
                generation: "generation_0".into()
            }
        );
        assert_eq!(
            normalized[1],
            Step::Admit {
                request: "request_0".into()
            }
        );
    }

    #[test]
    fn malformed_initial_and_unknown_invariant_are_errors() {
        let model = BrowserRequestModel;
        assert!(matches!(
            model.init(&serde_json::json!({"unknown": true})),
            Err(ScenarioError::InvalidInitial(_))
        ));
        let state = model.init(&serde_json::json!({})).unwrap();
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-999"), &state),
            Err(ScenarioError::UnknownInvariant(_))
        ));
    }

    #[test]
    fn every_catalogued_invariant_detects_an_independent_corruption() {
        let model = BrowserRequestModel;
        let mut state = BrowserRequestState::default();
        state.terminal_writes.insert("duplicate".into(), 2);
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-001"), &state).unwrap(),
            InvariantResult::Violated { .. }
        ));

        state = BrowserRequestState::default();
        state.active.insert(
            "unclean".into(),
            ActiveRequest {
                generation: "g".into(),
                phase: RequestPhase::Dispatched,
            },
        );
        state
            .terminal
            .insert("unclean".into(), TerminalOutcome::Success);
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-002"), &state).unwrap(),
            InvariantResult::Violated { .. }
        ));

        state = BrowserRequestState::default();
        state
            .first_terminal
            .insert("changed".into(), TerminalOutcome::TimedOut);
        state
            .terminal
            .insert("changed".into(), TerminalOutcome::Success);
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-003"), &state).unwrap(),
            InvariantResult::Violated { .. }
        ));

        state = BrowserRequestState::default();
        state
            .request_generations
            .insert("stale".into(), "old".into());
        state
            .completion_generations
            .insert("stale".into(), "new".into());
        state.completion_was_current.insert("stale".into(), false);
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-004"), &state).unwrap(),
            InvariantResult::Violated { .. }
        ));

        state = BrowserRequestState::default();
        state
            .terminal
            .insert("early".into(), TerminalOutcome::CancelledBeforeDispatch);
        state.dispatched.insert("early".into());
        assert!(matches!(
            model.check(&invariant("LABBY-REQ-005"), &state).unwrap(),
            InvariantResult::Violated { .. }
        ));
    }

    #[test]
    fn connection_generations_are_never_reused() {
        let model = BrowserRequestModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::Connect {
                        generation: "same".into()
                    }
                )
                .unwrap(),
            StepOutcome::Applied {}
        ));
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::ReplaceConnection {
                        generation: "same".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
    }

    #[test]
    fn timeout_requires_successful_dispatch() {
        let model = BrowserRequestModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [
            Step::Connect {
                generation: "socket".into(),
            },
            Step::Admit {
                request: "call".into(),
            },
        ] {
            assert_eq!(
                model.apply(&mut state, &step).unwrap(),
                StepOutcome::Applied {}
            );
        }

        let admitted = state.clone();
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::Timeout {
                        request: "call".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
        assert_eq!(state, admitted, "rejected timeout must not mutate state");

        assert_eq!(
            model
                .apply(
                    &mut state,
                    &Step::Dispatch {
                        request: "call".into()
                    }
                )
                .unwrap(),
            StepOutcome::Applied {}
        );
        assert_eq!(
            model
                .apply(
                    &mut state,
                    &Step::Timeout {
                        request: "call".into()
                    }
                )
                .unwrap(),
            StepOutcome::Applied {}
        );
        assert_eq!(state.terminal["call"], TerminalOutcome::TimedOut);
    }

    #[test]
    fn pre_dispatch_failure_never_credits_page_dispatch() {
        let model = BrowserRequestModel;
        let mut state = model.init(&serde_json::json!({})).unwrap();
        for step in [
            Step::Connect {
                generation: "generation".into(),
            },
            Step::Admit {
                request: "request".into(),
            },
            Step::FailBeforeDispatch {
                request: "request".into(),
            },
        ] {
            assert!(matches!(
                model.apply(&mut state, &step).unwrap(),
                StepOutcome::Applied {}
            ));
        }
        assert!(!state.dispatched.contains("request"));
        assert_eq!(state.terminal.get("request"), Some(&TerminalOutcome::Error));
        assert!(matches!(
            model
                .apply(
                    &mut state,
                    &Step::FailBeforeDispatch {
                        request: "request".into()
                    }
                )
                .unwrap(),
            StepOutcome::Rejected { .. }
        ));
    }

    #[test]
    fn canonicalization_does_not_turn_invalid_empty_ids_into_valid_ids() {
        let model = BrowserRequestModel;
        let (_, steps) = model
            .canonicalize(
                &serde_json::json!({}),
                &[
                    Step::Connect {
                        generation: String::new(),
                    },
                    Step::Admit {
                        request: String::new(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Connect {
                    generation: String::new()
                },
                Step::Admit {
                    request: String::new()
                }
            ]
        );
    }
}
