//! Real-process lifecycle observations; model evidence is never product evidence.
#![cfg(feature = "gateway")]
#![allow(dead_code, clippy::panic)]

#[path = "support/conformance_fixture.rs"]
mod conformance_fixture;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use conformance_fixture::{
    AdmissionBarrier, ConformanceFixture, DispatchedCall, HttpTerminal, InvocationAudit,
    PendingHttpCall,
};
use labby_model::{BrowserRequestModel, BrowserRequestState, RequestPhase, Step, TerminalOutcome};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use verify_core::{
    ConformanceFailure, ConformanceTarget, ScenarioError, StepOutcome, compare_trace,
};

struct PublicTarget;

struct DivergentPublicTarget;

struct PublicState {
    fixture: ConformanceFixture,
    pending: BTreeMap<String, PendingHttpCall>,
    calls: BTreeMap<String, DispatchedCall>,
    admission_barriers: BTreeMap<String, AdmissionBarrier>,
    terminal_audits: BTreeMap<String, InvocationAudit>,
    observation: Observation,
}

impl std::fmt::Debug for PublicState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicState")
            .field("observation", &self.observation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default)]
struct Observation {
    generation: Option<String>,
    active: BTreeMap<String, RequestPhase>,
    dispatched: BTreeSet<String>,
    terminals: BTreeMap<String, TerminalOutcome>,
}

fn harness(reason: String) -> ScenarioError {
    ScenarioError::Harness(reason)
}

impl PublicState {
    async fn retain_terminal_audit(
        &mut self,
        request: &str,
        outcome: &str,
        error_kind: Option<&str>,
    ) -> Result<(), ScenarioError> {
        let audits = self
            .fixture
            .wait_audit_count(self.terminal_audits.len() + 1)
            .await
            .map_err(harness)?;
        let mut new = audits
            .into_iter()
            .filter(|audit| !self.terminal_audits.values().any(|old| old.id == audit.id));
        let audit = new
            .next()
            .ok_or_else(|| harness("terminal audit was not unique".into()))?;
        if new.next().is_some()
            || audit.outcome != outcome
            || audit.error_kind.as_deref() != error_kind
        {
            return Err(harness(
                "persisted terminal audit differed from observed outcome".into(),
            ));
        }
        self.terminal_audits.insert(request.into(), audit);
        Ok(())
    }

    async fn start() -> Result<Self, ScenarioError> {
        let fixture = tokio::time::timeout(
            Duration::from_secs(30),
            ConformanceFixture::start(Duration::from_secs(15)),
        )
        .await
        .map_err(|_| harness("fixture setup deadline".into()))?
        .map_err(harness)?;
        let observation = Observation {
            generation: Some(fixture.generation_label()),
            ..Observation::default()
        };
        Ok(PublicState {
            fixture,
            pending: BTreeMap::new(),
            calls: BTreeMap::new(),
            admission_barriers: BTreeMap::new(),
            terminal_audits: BTreeMap::new(),
            observation,
        })
    }
}

impl ConformanceTarget<BrowserRequestModel> for PublicTarget {
    type ImplementationState = PublicState;
    type Observation = Observation;

    async fn apply(
        &self,
        state: &mut PublicState,
        step: &Step,
    ) -> Result<StepOutcome, ScenarioError> {
        match step {
            Step::ReplaceConnection { generation } => {
                if state.pending.len() > 1 {
                    return Err(harness(
                        "replacement fixture supports one active call".into(),
                    ));
                }
                if let Some(request) = state.pending.keys().next().cloned() {
                    let call = state
                        .calls
                        .get(&request)
                        .ok_or_else(|| harness("replacement call was not admitted".into()))?;
                    let label = state
                        .fixture
                        .replace_connection_with_pending(call)
                        .await
                        .map_err(harness)?;
                    if label != *generation {
                        return Err(harness("replacement generation label differed".into()));
                    }
                    let pending = state
                        .pending
                        .remove(&request)
                        .expect("selected pending call");
                    let terminal = state
                        .fixture
                        .wait_http_terminal(pending)
                        .await
                        .map_err(harness)?;
                    if !matches!(terminal, HttpTerminal::Error { ref kind, .. } if kind == "browser_offline")
                    {
                        return Err(harness("replacement did not terminalize prior call".into()));
                    }
                    let audits = state
                        .fixture
                        .wait_audit_count(state.terminal_audits.len() + 1)
                        .await
                        .map_err(harness)?;
                    let audit = audits
                        .into_iter()
                        .find(|audit| !state.terminal_audits.values().any(|old| old.id == audit.id))
                        .ok_or_else(|| harness("replacement audit was not unique".into()))?;
                    if audit.outcome != "failed"
                        || audit.error_kind.as_deref() != Some("browser_offline")
                    {
                        return Err(harness("replacement audit terminal differed".into()));
                    }
                    state.terminal_audits.insert(request.clone(), audit);
                    state
                        .observation
                        .terminals
                        .insert(request.clone(), TerminalOutcome::BrowserOffline);
                    state.observation.active.remove(&request);
                } else {
                    let label = state.fixture.replace_connection().await.map_err(harness)?;
                    if label != *generation {
                        return Err(harness("replacement generation label differed".into()));
                    }
                }
                state.observation.generation = Some(generation.clone());
            }
            Step::Admit { request } => {
                let barrier = state.fixture.hold_audit_writes().map_err(harness)?;
                state.pending.insert(
                    request.clone(),
                    state.fixture.begin_call_with_timeout(
                        json!({"fixture":true}),
                        Duration::from_millis(750),
                    ),
                );
                let pending = state
                    .pending
                    .get_mut(request)
                    .ok_or_else(|| harness("missing fixture request".into()))?;
                let call = state
                    .fixture
                    .wait_admitted(pending)
                    .await
                    .map_err(harness)?;
                state.calls.insert(request.clone(), call);
                state.admission_barriers.insert(request.clone(), barrier);
                state
                    .observation
                    .active
                    .insert(request.clone(), RequestPhase::Admitted);
            }
            Step::Dispatch { request } => {
                state
                    .admission_barriers
                    .remove(request)
                    .ok_or_else(|| harness("missing admission barrier".into()))?
                    .release()
                    .map_err(harness)?;
                let pending = state
                    .pending
                    .get_mut(request)
                    .ok_or_else(|| harness("missing fixture request".into()))?;
                let call = state
                    .fixture
                    .wait_dispatch(pending)
                    .await
                    .map_err(harness)?;
                let admitted = state
                    .calls
                    .get(request)
                    .ok_or_else(|| harness("missing admitted call".into()))?;
                if admitted.call_id != call.call_id {
                    return Err(harness("dispatch did not match admitted call".into()));
                }
                state.observation.dispatched.insert(request.clone());
                state
                    .observation
                    .active
                    .insert(request.clone(), RequestPhase::Dispatched);
            }
            Step::CompleteSuccess {
                request,
                generation,
            } => {
                let call = state
                    .calls
                    .get(request)
                    .ok_or_else(|| harness("missing dispatched call".into()))?;
                if call.generation_label != *generation {
                    return Err(harness(
                        "stale-generation injection is not implemented by this adapter".into(),
                    ));
                }
                if state.observation.terminals.contains_key(request) {
                    // A generic completion acknowledgement does not expose its
                    // acceptance status. Do not manufacture a Rejected result.
                    state
                        .fixture
                        .send_late_completion(call, json!({"late":true}))
                        .await
                        .map_err(harness)?;
                    let _retained = state
                        .terminal_audits
                        .get(request)
                        .ok_or_else(|| harness("terminal audit was not retained".into()))?;
                    let audits = state.fixture.audit_snapshot().map_err(harness)?;
                    if audits.len() != state.terminal_audits.len()
                        || !state
                            .terminal_audits
                            .values()
                            .all(|audit| audits.contains(audit))
                    {
                        return Err(harness("late completion changed the terminal audit".into()));
                    }
                    return Ok(StepOutcome::Rejected {
                        reason: "late response has no active request".into(),
                    });
                }
                state
                    .fixture
                    .complete_success(call, json!({"accepted":true}))
                    .await
                    .map_err(harness)?;
                let pending = state
                    .pending
                    .remove(request)
                    .ok_or_else(|| harness("missing pending caller".into()))?;
                let terminal = state
                    .fixture
                    .wait_http_terminal(pending)
                    .await
                    .map_err(harness)?;
                if terminal != HttpTerminal::Success(json!({"accepted":true})) {
                    return Err(harness("unexpected public HTTP terminal".into()));
                }
                state
                    .observation
                    .terminals
                    .insert(request.clone(), TerminalOutcome::Success);
                state.observation.active.remove(request);
                let audits = state
                    .fixture
                    .wait_audit_count(state.terminal_audits.len() + 1)
                    .await
                    .map_err(harness)?;
                let audit = audits
                    .into_iter()
                    .find(|audit| !state.terminal_audits.values().any(|old| old.id == audit.id))
                    .ok_or_else(|| harness("success audit was not unique".into()))?;
                if audit.outcome != "succeeded" || audit.error_kind.is_some() {
                    return Err(harness("success audit terminal differed".into()));
                }
                state.terminal_audits.insert(request.clone(), audit);
            }
            Step::CompleteError {
                request,
                generation,
            } => {
                let call = state
                    .calls
                    .get(request)
                    .ok_or_else(|| harness("missing dispatched call".into()))?;
                if call.generation_label != *generation {
                    return Err(harness("stale error completion is not controllable".into()));
                }
                state
                    .fixture
                    .complete_error(call, "fixture_error", "controlled error")
                    .await
                    .map_err(harness)?;
                let pending = state
                    .pending
                    .remove(request)
                    .ok_or_else(|| harness("missing pending caller".into()))?;
                let terminal = state
                    .fixture
                    .wait_http_terminal(pending)
                    .await
                    .map_err(harness)?;
                if !matches!(terminal, HttpTerminal::Error { ref kind, .. } if kind == "invalid_request")
                {
                    return Err(harness("error completion terminal differed".into()));
                }
                state
                    .observation
                    .terminals
                    .insert(request.clone(), TerminalOutcome::Error);
                state.observation.active.remove(request);
                state
                    .retain_terminal_audit(request, "failed", Some("invalid_request"))
                    .await?;
            }
            Step::Cancel { request } => {
                let call = state.calls.get(request).ok_or_else(|| harness("pre-dispatch cancellation cannot be controlled through this public boundary".into()))?;
                let pending = state
                    .pending
                    .get_mut(request)
                    .ok_or_else(|| harness("missing pending caller".into()))?;
                ConformanceFixture::abort_caller(pending)
                    .await
                    .map_err(harness)?;
                state.fixture.wait_cancel(call).await.map_err(harness)?;
                let before_dispatch = !state.observation.dispatched.contains(request);
                if let Some(barrier) = state.admission_barriers.remove(request) {
                    barrier.release().map_err(harness)?;
                }
                let audits = state
                    .fixture
                    .wait_audit_count(state.terminal_audits.len() + 1)
                    .await
                    .map_err(harness)?;
                let audit = audits
                    .into_iter()
                    .find(|audit| !state.terminal_audits.values().any(|old| old.id == audit.id))
                    .ok_or_else(|| harness("cancellation audit was not unique".into()))?;
                if audit.outcome != "abandoned"
                    || audit.error_kind.as_deref() != Some("caller_cancelled")
                {
                    return Err(harness("cancellation audit terminal differed".into()));
                }
                state.terminal_audits.insert(request.clone(), audit);
                state.observation.terminals.insert(
                    request.clone(),
                    if before_dispatch {
                        TerminalOutcome::CancelledBeforeDispatch
                    } else {
                        TerminalOutcome::CancelledAfterDispatch
                    },
                );
                state.observation.active.remove(request);
            }
            Step::Timeout { request } => {
                let call = state
                    .calls
                    .get(request)
                    .ok_or_else(|| harness("missing dispatched call".into()))?;
                state.fixture.wait_cancel(call).await.map_err(harness)?;
                let pending = state
                    .pending
                    .remove(request)
                    .ok_or_else(|| harness("missing pending caller".into()))?;
                let terminal = state
                    .fixture
                    .wait_http_terminal(pending)
                    .await
                    .map_err(harness)?;
                if !matches!(terminal, HttpTerminal::Error { ref kind, .. } if kind == "tool_timeout")
                {
                    return Err(harness("timeout terminal differed".into()));
                }
                state
                    .observation
                    .terminals
                    .insert(request.clone(), TerminalOutcome::TimedOut);
                state.observation.active.remove(request);
                state
                    .retain_terminal_audit(request, "failed", Some("tool_timeout"))
                    .await?;
            }
            Step::Disconnect { generation } => {
                if state.observation.generation.as_ref() != Some(generation) {
                    return Err(harness(
                        "stale disconnect injection is not implemented".into(),
                    ));
                }
                state.fixture.disconnect().await.map_err(harness)?;
                for (request, pending) in std::mem::take(&mut state.pending) {
                    let terminal = state
                        .fixture
                        .wait_http_terminal(pending)
                        .await
                        .map_err(harness)?;
                    if !matches!(terminal, HttpTerminal::Error { ref kind, .. } if kind == "browser_offline")
                    {
                        return Err(harness("disconnect did not produce browser_offline".into()));
                    }
                    state
                        .observation
                        .terminals
                        .insert(request.clone(), TerminalOutcome::BrowserOffline);
                    state
                        .retain_terminal_audit(&request, "failed", Some("browser_offline"))
                        .await?;
                }
                state.observation.generation = None;
                state.observation.active.clear();
            }
            Step::InvalidateDocument { request } => {
                let call = state
                    .calls
                    .get(request)
                    .ok_or_else(|| harness("missing dispatched call".into()))?;
                state
                    .fixture
                    .invalidate_document(call)
                    .await
                    .map_err(harness)?;
                let pending = state
                    .pending
                    .remove(request)
                    .ok_or_else(|| harness("missing pending caller".into()))?;
                let terminal = state
                    .fixture
                    .wait_http_terminal(pending)
                    .await
                    .map_err(harness)?;
                if !matches!(terminal, HttpTerminal::Error { ref kind, .. } if kind == "stale_document")
                {
                    return Err(harness("document invalidation terminal differed".into()));
                }
                state
                    .observation
                    .terminals
                    .insert(request.clone(), TerminalOutcome::StaleDocument);
                state.observation.active.remove(request);
                state
                    .retain_terminal_audit(request, "failed", Some("stale_document"))
                    .await?;
            }
            _ => {
                return Err(harness(
                    "controlled step is not implemented by this public adapter".into(),
                ));
            }
        }
        Ok(StepOutcome::Applied {})
    }

    async fn observe(&self, state: &PublicState) -> Result<Observation, ScenarioError> {
        Ok(state.observation.clone())
    }

    fn observations_conform(
        &self,
        model: &BrowserRequestState,
        actual: &Observation,
    ) -> Result<(), String> {
        if model.current_generation != actual.generation {
            return Err("authenticated socket generation differs".into());
        }
        let model_active: BTreeMap<_, _> = model
            .active
            .iter()
            .map(|(request, active)| (request.clone(), active.phase.clone()))
            .collect();
        if model_active != actual.active {
            return Err("public active request phases differ".into());
        }
        if model.dispatched != actual.dispatched {
            return Err("public dispatch observations differ".into());
        }
        if model.terminal != actual.terminals {
            return Err("public terminal observations differ".into());
        }
        Ok(())
    }
}

impl ConformanceTarget<BrowserRequestModel> for DivergentPublicTarget {
    type ImplementationState = PublicState;
    type Observation = Observation;

    async fn apply(
        &self,
        state: &mut PublicState,
        step: &Step,
    ) -> Result<StepOutcome, ScenarioError> {
        PublicTarget.apply(state, step).await
    }

    async fn observe(&self, state: &PublicState) -> Result<Observation, ScenarioError> {
        PublicTarget.observe(state).await
    }

    fn observations_conform(
        &self,
        model: &BrowserRequestState,
        actual: &Observation,
    ) -> Result<(), String> {
        PublicTarget.observations_conform(model, actual)?;
        if actual
            .terminals
            .values()
            .any(|terminal| *terminal == TerminalOutcome::Success)
        {
            return Err("negative adapter deliberately rejects an observed success".into());
        }
        Ok(())
    }
}

async fn run_conformance_case(case_id: &str, steps: Vec<Step>, negative: bool) {
    let mut state = PublicState::start().await.expect("real process startup");
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        if negative {
            compare_trace(
                &BrowserRequestModel,
                &DivergentPublicTarget,
                &mut state,
                &json!({"current_generation":"fixture-generation-1"}),
                &steps,
            )
            .await
        } else {
            compare_trace(
                &BrowserRequestModel,
                &PublicTarget,
                &mut state,
                &json!({"current_generation":"fixture-generation-1"}),
                &steps,
            )
            .await
        }
    })
    .await;
    let identity = state.fixture.identity();
    let incident_input = state.fixture.incident_input();
    let public_events: Vec<_> = state
        .fixture
        .evidence()
        .iter()
        .map(|event| format!("{event:?}"))
        .collect();
    // Release any admission lock even when comparison timed out or failed.
    state.admission_barriers.clear();
    state.pending.clear();
    let cleanup = state.fixture.finish().await;
    let passed = result.as_ref().is_ok_and(|trace| {
        if negative {
            matches!(
                trace.failure,
                Some(ConformanceFailure::ObservationDivergence { position: 3, .. })
            ) && trace.observations.len() == 4
        } else {
            trace.conforms() && trace.observations.len() == steps.len() + 1
        }
    }) && cleanup.is_clean()
        && incident_input.is_ok();
    if let Some(directory) = std::env::var_os("LABBY_CONFORMANCE_EVIDENCE_DIR") {
        use sha2::{Digest as _, Sha256};
        let mut canonical_steps = serde_json::to_value(&steps).expect("controlled trace");
        for step in canonical_steps.as_array_mut().expect("step array") {
            step.as_object_mut().expect("step object").sort_keys();
        }
        let encoded = serde_json::to_vec(&canonical_steps).expect("canonical trace");
        let observations: Vec<_> = result.as_ref().map(|trace| trace.observations.iter()
            .map(|observation| json!({
                "position":observation.position,
                "relation_passed":matches!(observation.relation,verify_core::RelationStatus::Conforms),
                "generation":observation.implementation.generation,
                "active":observation.implementation.active,
                "dispatched":observation.implementation.dispatched,
                "terminals":observation.implementation.terminals,
                "model_outcome":observation.model_outcome.map(|outcome| format!("{outcome:?}")),
                "implementation_outcome":observation.implementation_outcome.map(|outcome| format!("{outcome:?}")),
            })).collect()).unwrap_or_default();
        let failure = match result.as_ref() {
            Ok(trace) => match &trace.failure {
                None => serde_json::Value::Null,
                Some(ConformanceFailure::ObservationDivergence { position, .. }) => {
                    json!({"kind":"observation_divergence","position":position})
                }
                Some(_) => json!({"kind":"comparison_failed"}),
            },
            Err(_) => json!({"kind":"deadline"}),
        };
        let report = json!({
            "schema_version":1,"lane":"conformance","case_id":case_id,
            "evidence_kind":if negative {"negative_adapter_self_test"} else {"real_process"},
            "relation":"browser-public-v1",
            "trace_fingerprint":Sha256::digest(&encoded).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "steps":canonical_steps,"public_events":public_events,
            "incident_input":incident_input.as_ref().ok(),
            "observations":observations,"failure":failure,
            "source":identity,"cleanup":{"clean":cleanup.is_clean(),"failures":cleanup.failures},
            "verdict":if passed {"passed"} else {"failed"}
        });
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).expect("evidence directory");
        let mut temporary =
            tempfile::NamedTempFile::new_in(&directory).expect("temporary evidence");
        serde_json::to_writer_pretty(temporary.as_file_mut(), &report).expect("evidence JSON");
        temporary.as_file().sync_all().expect("flush evidence");
        temporary
            .persist(directory.join(format!("{case_id}.json")))
            .expect("publish evidence");
    }
    assert!(cleanup.is_clean(), "cleanup: {cleanup:?}");
    incident_input.expect("bounded real daemon lifecycle capture after cleanup");
    let trace = result.expect("conformance deadline after explicit cleanup");
    assert!(passed, "case {case_id}: {:?}", trace.failure);
}

fn dispatched_prefix() -> Vec<Step> {
    vec![
        Step::Admit {
            request: "request-1".into(),
        },
        Step::Dispatch {
            request: "request-1".into(),
        },
    ]
}

#[tokio::test]
async fn conformance_negative_real_adapter_is_detected() {
    let mut steps = dispatched_prefix();
    steps.push(Step::CompleteSuccess {
        request: "request-1".into(),
        generation: "fixture-generation-1".into(),
    });
    run_conformance_case("divergent-adapter-self-test", steps, true).await;
}

#[tokio::test]
async fn conformance_real_success_and_disconnect() {
    for (case_id, terminal) in [
        (
            "success",
            Step::CompleteSuccess {
                request: "request-1".into(),
                generation: "fixture-generation-1".into(),
            },
        ),
        (
            "disconnect",
            Step::Disconnect {
                generation: "fixture-generation-1".into(),
            },
        ),
    ] {
        let mut steps = dispatched_prefix();
        steps.push(terminal);
        run_conformance_case(case_id, steps, false).await;
    }
}

#[tokio::test]
async fn conformance_real_error_timeout_and_document_invalidation() {
    for (case_id, terminal) in [
        (
            "tool-error",
            Step::CompleteError {
                request: "request-1".into(),
                generation: "fixture-generation-1".into(),
            },
        ),
        (
            "timeout",
            Step::Timeout {
                request: "request-1".into(),
            },
        ),
        (
            "document-invalidation",
            Step::InvalidateDocument {
                request: "request-1".into(),
            },
        ),
    ] {
        let mut steps = dispatched_prefix();
        steps.push(terminal);
        run_conformance_case(case_id, steps, false).await;
    }
}

#[tokio::test]
async fn conformance_replacement_terminalizes_old_generation_and_owns_new_calls() {
    run_conformance_case(
        "generation-replacement",
        vec![
            Step::Admit {
                request: "old".into(),
            },
            Step::Dispatch {
                request: "old".into(),
            },
            Step::ReplaceConnection {
                generation: "fixture-generation-2".into(),
            },
            Step::Admit {
                request: "new".into(),
            },
            Step::Dispatch {
                request: "new".into(),
            },
            Step::CompleteSuccess {
                request: "new".into(),
                generation: "fixture-generation-2".into(),
            },
        ],
        false,
    )
    .await;
}

#[tokio::test]
async fn conformance_pre_dispatch_cancellation_and_late_terminal_are_observed() {
    run_conformance_case(
        "admit-cancel-late-cleanup",
        vec![
            Step::Admit {
                request: "request-1".into(),
            },
            Step::Cancel {
                request: "request-1".into(),
            },
            Step::CompleteSuccess {
                request: "request-1".into(),
                generation: "fixture-generation-1".into(),
            },
        ],
        false,
    )
    .await;
}

#[tokio::test]
async fn conformance_cancellation_retains_terminal_after_late_completion() {
    let mut steps = dispatched_prefix();
    steps.extend([
        Step::Cancel {
            request: "request-1".into(),
        },
        Step::CompleteSuccess {
            request: "request-1".into(),
            generation: "fixture-generation-1".into(),
        },
    ]);
    run_conformance_case("dispatch-cancel-late-cleanup", steps, false).await;
}
