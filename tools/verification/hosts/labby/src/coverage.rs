use std::{collections::BTreeSet, time::Instant};

use labby_model::{
    BrowserRequestModel, CAPABILITY_VISIBILITY_MODEL, CapabilityPhase, CapabilityVisibilityModel,
    CapabilityVisibilityStep, MODEL, Step, TerminalOutcome,
};
use verify_core::{ScenarioTarget, StepOutcome};
use verify_scenario::ValidatedScenario;

pub(crate) fn has_semantic_witness(
    scenario: &ValidatedScenario,
    deadline: Instant,
) -> Result<bool, &'static str> {
    match scenario.scenario().model.as_str() {
        MODEL => browser_request_witness(scenario, deadline),
        CAPABILITY_VISIBILITY_MODEL => capability_visibility_witness(scenario, deadline),
        _ => Ok(false),
    }
}

fn browser_request_witness(
    scenario: &ValidatedScenario,
    deadline: Instant,
) -> Result<bool, &'static str> {
    let raw = scenario.scenario();
    let Ok(steps) = raw
        .steps
        .iter()
        .cloned()
        .map(serde_json::from_value::<Step>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Ok(false);
    };
    let model = BrowserRequestModel;
    let Ok(mut state) = model.init(&serde_json::Value::Object(raw.initial.clone())) else {
        return Ok(false);
    };
    let mut admitted = BTreeSet::new();
    let mut dispatched = BTreeSet::new();
    let mut terminalized = BTreeSet::new();
    let mut inactive_after_terminal = BTreeSet::new();
    let mut late_completion_rejected = false;
    let mut stale_completion_requests = BTreeSet::new();
    let mut owned_completion_requests = BTreeSet::new();
    let mut cancelled_before_dispatch = BTreeSet::new();
    let mut cancelled_request_blocked = false;

    for step in &steps {
        if Instant::now() >= deadline {
            return Err("semantic witness deadline exceeded");
        }
        let current_generation = state.current_generation.clone();
        let request = request_id(step);
        let terminal_before = request.and_then(|id| state.terminal.get(id).cloned());
        let writes_before = request.and_then(|id| state.terminal_writes.get(id).copied());
        let live_before = request.is_some_and(|id| state.active.contains_key(id));
        let implicitly_terminalized = owned_requests_terminalized_by(step, &state);
        let Ok(outcome) = model.apply(&mut state, step) else {
            return Ok(false);
        };
        if Instant::now() >= deadline {
            return Err("semantic witness deadline exceeded");
        }
        let applied = matches!(outcome, StepOutcome::Applied {});
        let rejected = matches!(outcome, StepOutcome::Rejected { .. });
        match step {
            Step::Admit { request } if applied => {
                admitted.insert(request.clone());
            }
            Step::Dispatch { request } if applied => {
                dispatched.insert(request.clone());
            }
            Step::CompleteSuccess {
                request,
                generation,
            }
            | Step::CompleteError {
                request,
                generation,
            } => {
                if rejected
                    && terminal_before.is_some()
                    && state.terminal.get(request) == terminal_before.as_ref()
                    && state.terminal_writes.get(request).copied() == writes_before
                {
                    late_completion_rejected = true;
                }
                if rejected
                    && live_before
                    && current_generation.is_some()
                    && current_generation.as_deref() != Some(generation)
                {
                    stale_completion_requests.insert(request.clone());
                }
                if applied && current_generation.as_deref() == Some(generation) {
                    owned_completion_requests.insert(request.clone());
                }
                if rejected && cancelled_before_dispatch.contains(request) {
                    cancelled_request_blocked = true;
                }
            }
            Step::Cancel { request } if applied => {
                if admitted.contains(request)
                    && !dispatched.contains(request)
                    && state.terminal.get(request)
                        == Some(&TerminalOutcome::CancelledBeforeDispatch)
                {
                    cancelled_before_dispatch.insert(request.clone());
                }
            }
            Step::Dispatch { request }
                if rejected && cancelled_before_dispatch.contains(request) =>
            {
                cancelled_request_blocked = true;
            }
            _ => {}
        }
        if applied {
            if let Some(request) = request
                && terminal_before.is_none()
                && state.terminal.contains_key(request)
                && admitted.contains(request)
            {
                terminalized.insert(request.to_owned());
                if !state.active.contains_key(request) {
                    inactive_after_terminal.insert(request.to_owned());
                }
            }
            for request in implicitly_terminalized {
                if state.terminal.contains_key(&request) && admitted.contains(&request) {
                    terminalized.insert(request.clone());
                    if !state.active.contains_key(&request) {
                        inactive_after_terminal.insert(request);
                    }
                }
            }
        }
    }
    Ok(match raw.invariant.as_str() {
        "LABBY-REQ-001" => !terminalized.is_empty(),
        "LABBY-REQ-002" => terminalized
            .iter()
            .any(|request| inactive_after_terminal.contains(request)),
        "LABBY-REQ-003" => late_completion_rejected,
        "LABBY-REQ-004" => stale_completion_requests
            .iter()
            .any(|request| owned_completion_requests.contains(request)),
        "LABBY-REQ-005" => !cancelled_before_dispatch.is_empty() && cancelled_request_blocked,
        _ => false,
    })
}

fn capability_visibility_witness(
    scenario: &ValidatedScenario,
    deadline: Instant,
) -> Result<bool, &'static str> {
    let raw = scenario.scenario();
    let Ok(steps) = raw
        .steps
        .iter()
        .cloned()
        .map(serde_json::from_value::<CapabilityVisibilityStep>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Ok(false);
    };
    let model = CapabilityVisibilityModel;
    let Ok(mut state) = model.init(&serde_json::Value::Object(raw.initial.clone())) else {
        return Ok(false);
    };
    for step in &steps {
        if Instant::now() >= deadline {
            return Err("semantic witness deadline exceeded");
        }
        if !matches!(model.apply(&mut state, step), Ok(StepOutcome::Applied {})) {
            return Ok(false);
        }
    }
    Ok(match raw.invariant.as_str() {
        "LABBY-CAP-001" | "LABBY-CAP-002" | "LABBY-CAP-003" => !state.degraded.is_empty(),
        "LABBY-CAP-004" => state.phase == CapabilityPhase::Blocked && state.fatal_guard.is_some(),
        _ => false,
    })
}

fn request_id(step: &Step) -> Option<&str> {
    match step {
        Step::Admit { request }
        | Step::Dispatch { request }
        | Step::FailBeforeDispatch { request }
        | Step::CompleteSuccess { request, .. }
        | Step::CompleteError { request, .. }
        | Step::Cancel { request }
        | Step::Timeout { request }
        | Step::InvalidateDocument { request } => Some(request),
        Step::Connect { .. } | Step::ReplaceConnection { .. } | Step::Disconnect { .. } => None,
    }
}

fn owned_requests_terminalized_by(
    step: &Step,
    state: &labby_model::BrowserRequestState,
) -> Vec<String> {
    let generation = match step {
        Step::ReplaceConnection { .. } => state.current_generation.as_deref(),
        Step::Disconnect { generation }
            if state.current_generation.as_deref() == Some(generation) =>
        {
            Some(generation.as_str())
        }
        _ => None,
    };
    generation.map_or_else(Vec::new, |generation| {
        state
            .active
            .iter()
            .filter(|(_, active)| active.generation == generation)
            .map(|(request, _)| request.clone())
            .collect()
    })
}
