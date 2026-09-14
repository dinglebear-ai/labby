//! Allowlisted incident reduction; raw log fields never enter scenario output.

use labby_model::{BrowserRequestModel, MODEL, Step};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{ffi::OsString, io::Write, num::NonZeroU64, path::Path};
use verify_core::{Backend, CheckPlan, InvariantId, ScenarioTarget};
use verify_runner::{ReplayLimits, ReplayReport, TargetRegistry, TraceVerdict};
use verify_scenario::{Expectation, Origin, OriginKind, Scenario, Status};

#[derive(Deserialize)]
struct Incident {
    schema: u32,
    generation: Option<String>,
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    step: Option<Step>,
    fields: Option<LifecycleFields>,
}

#[derive(Deserialize)]
struct LifecycleFields {
    action: Option<String>,
    phase: Option<String>,
    generation_id: Option<String>,
    previous_generation_id: Option<String>,
    call_id: Option<String>,
}

fn lifecycle_steps(incident: Incident) -> Result<(Vec<Step>, bool), String> {
    let Some(root) = incident.generation else {
        let steps = incident
            .events
            .into_iter()
            .map(|event| {
                if event.fields.is_some() {
                    return Err(String::from(
                        "structured logs require an explicit root generation",
                    ));
                }
                event
                    .step
                    .ok_or_else(|| "incident event lacks a modeled step".into())
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((steps, false));
    };
    let mut generations = std::collections::BTreeSet::new();
    let mut current_generation = None;
    let mut steps = Vec::new();
    for event in incident.events {
        if event.step.is_some() {
            return Err("incident cannot mix modeled steps and lifecycle logs".into());
        }
        let Some(fields) = event.fields else {
            continue;
        };
        if fields.action.as_deref() != Some("browser.call.lifecycle") {
            continue;
        }
        let generation = fields
            .generation_id
            .ok_or("lifecycle event lacks generation")?;
        let phase = fields
            .phase
            .as_deref()
            .ok_or("lifecycle event lacks phase")?;
        if phase == "connected" && generation == root && generations.is_empty() {
            generations.insert(generation.clone());
            current_generation = Some(generation.clone());
            steps.push(Step::Connect { generation });
            continue;
        }
        if generation == root && generations.is_empty() {
            return Err("incident lacks the selected connection start".into());
        }
        if phase == "replaced"
            && fields
                .previous_generation_id
                .as_ref()
                .is_some_and(|old| generations.contains(old))
        {
            if fields.previous_generation_id != current_generation {
                return Err("inconsistent lifecycle replacement lineage".into());
            }
            if !generations.insert(generation.clone()) {
                return Err("lifecycle generation reused".into());
            }
            current_generation = Some(generation.clone());
            steps.push(Step::ReplaceConnection { generation });
            continue;
        }
        if !generations.contains(&generation) {
            continue;
        }
        if phase == "disconnected" {
            if current_generation.as_ref() == Some(&generation) {
                current_generation = None;
            }
            steps.push(Step::Disconnect { generation });
            continue;
        }
        let request = fields
            .call_id
            .ok_or("lifecycle call event lacks call identity")?;
        steps.push(match phase {
            "admitted" => Step::Admit { request },
            "dispatched" => Step::Dispatch { request },
            "persistence_failed" | "revalidation_failed" | "dispatch_failed" => {
                Step::FailBeforeDispatch { request }
            }
            "succeeded" => Step::CompleteSuccess {
                request,
                generation,
            },
            "failed" => Step::CompleteError {
                request,
                generation,
            },
            "cancelled" => Step::Cancel { request },
            "timed_out" => Step::Timeout { request },
            "invalidated" => Step::InvalidateDocument { request },
            _ => return Err("unsupported lifecycle phase".into()),
        });
    }
    if steps.is_empty() {
        return Err("incident lacks the selected connection start".into());
    }
    Ok((steps, true))
}

/// Redacted scenario plus independent reproduction result. This is model
/// evidence only; an unreproduced incident is never a passing product test.
#[derive(Serialize)]
pub struct ReducedIncident {
    /// Canonicalized, allowlisted scenario with no raw log payload.
    pub scenario: Scenario,
    /// Replay observations against the current lifecycle model.
    pub replay: ReplayReport,
}

/// Reduce structured lifecycle records. Unknown outer log fields are discarded;
/// malformed/unsupported steps fail with a static, non-payload diagnostic.
pub fn reduce(input: &str, invariant: &str) -> Result<ReducedIncident, String> {
    if input.len() > 1_048_576 {
        return Err("incident input exceeds one MiB".into());
    }
    let incident: Incident =
        serde_json::from_str(input).map_err(|_| "invalid structured lifecycle incident")?;
    if incident.schema != 1 || incident.events.is_empty() || incident.events.len() > 256 {
        return Err("incident requires schema 1 and 1..256 lifecycle events".into());
    }
    let id =
        InvariantId::try_from(invariant.to_owned()).map_err(|_| "invalid invariant identity")?;
    let model = BrowserRequestModel;
    let initial = model
        .init(&json!({}))
        .map_err(|_| "invalid initial model")?;
    model
        .check(&id, &initial)
        .map_err(|_| "unknown lifecycle invariant")?;
    let (steps, structured) = lifecycle_steps(incident)?;
    // Names are opaque source identifiers: canonicalize rather than preserving
    // strings that could contain credentials, paths, subjects or other PII.
    let (_, canonical) = model
        .canonicalize(&json!({}), &steps)
        .map_err(|_| "cannot canonicalize incident")?;
    if canonical.len() != steps.len() {
        return Err("canonicalization changes incident length".into());
    }
    let mut original_state = initial.clone();
    let mut canonical_state = initial;
    for (original, renamed) in steps.iter().zip(&canonical) {
        let original = model
            .apply(&mut original_state, original)
            .map_err(|_| "cannot replay incident source")?;
        let renamed = model
            .apply(&mut canonical_state, renamed)
            .map_err(|_| "cannot replay canonical incident")?;
        if structured
            && (!matches!(original, verify_core::StepOutcome::Applied { .. })
                || !matches!(renamed, verify_core::StepOutcome::Applied { .. }))
        {
            return Err("structured lifecycle transition is out of order".into());
        }
        if std::mem::discriminant(&original) != std::mem::discriminant(&renamed) {
            return Err("canonicalization changes lifecycle behavior".into());
        }
    }
    let steps = canonical;
    let mut scenario = Scenario {
        schema: 1,
        project: "labby".into(),
        model: MODEL.into(),
        invariant: id,
        origin: Origin {
            kind: OriginKind::Incident,
            tool_version: Some(env!("CARGO_PKG_VERSION").into()),
            discovered_at: None,
            seed: None,
            reference: Some(format!(
                "source:b3:{}",
                blake3::hash(input.as_bytes()).to_hex()
            )),
        },
        bounds: Default::default(),
        initial: Default::default(),
        steps: steps
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|_| "cannot encode incident")?,
        expect: Expectation::InvariantViolated,
        status: Status::Unreproduced,
        fingerprint: None,
    };
    let mut registry = TargetRegistry::default();
    let backend = crate::stateright::backend()?;
    let kani = crate::kani_backend()?;
    let mut backends = verify_core::BackendRegistry::default();
    backends
        .register(&backend)
        .map_err(|_| "cannot register backend")?;
    backends
        .register(&kani)
        .map_err(|_| "cannot register Kani backend")?;
    let catalog = labby_model::catalog(&backends).map_err(|_| "invalid lifecycle catalog")?;
    registry
        .register(&catalog, MODEL, model)
        .map_err(|_| "cannot register lifecycle model")?;
    let validated = scenario
        .clone()
        .validate()
        .map_err(|_| "invalid reduced scenario")?;
    let replay = registry.replay(&validated, &ReplayLimits::default());
    if replay.verdict == TraceVerdict::InvariantViolated {
        scenario.status = Status::Active;
    }
    // Replay again with the selected lifecycle so gate metadata is consistent.
    let validated = scenario
        .clone()
        .validate()
        .map_err(|_| "invalid reduced scenario")?;
    let replay = registry.replay(&validated, &ReplayLimits::default());
    scenario.fingerprint = Some(validated.fingerprint());
    Ok(ReducedIncident { scenario, replay })
}

/// Explore the finite state domain after a reduced incident prefix. Prefix
/// transitions remain in projected counterexamples and consume the depth bound.
pub fn explore(reduced: &ReducedIncident) -> Result<verify_core::BackendReport, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    reduced
        .scenario
        .clone()
        .validate()
        .map_err(|_| "invalid reduced scenario")?;
    if reduced.scenario.project != "labby"
        || reduced.scenario.model != MODEL
        || reduced.scenario.origin.kind != OriginKind::Incident
        || reduced.scenario.expect != Expectation::InvariantViolated
        || !reduced.scenario.initial.is_empty()
    {
        return Err("incident exploration requires a Labby lifecycle incident".into());
    }
    let steps: Vec<Step> = reduced
        .scenario
        .steps
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|_| "invalid reduced prefix")?;
    let (_, canonical) = BrowserRequestModel
        .canonicalize(&json!({}), &steps)
        .map_err(|_| "invalid reduced prefix")?;
    if steps != canonical {
        return Err("incident exploration requires canonical identifiers".into());
    }
    let backend = crate::stateright::backend_from_prefix(&steps)?;
    let kani = crate::kani_backend()?;
    let mut backends = verify_core::BackendRegistry::default();
    backends
        .register(&backend)
        .map_err(|_| "cannot register incident backend")?;
    backends
        .register(&kani)
        .map_err(|_| "cannot register Kani backend")?;
    let catalog = labby_model::catalog(&backends).map_err(|_| "invalid lifecycle catalog")?;
    let invariant = catalog
        .catalog()
        .invariant
        .iter()
        .find(|item| item.id == reduced.scenario.invariant)
        .ok_or("unknown lifecycle invariant")?;
    let handle = invariant
        .checks
        .get(&backend.id())
        .and_then(|handles| handles.0.first())
        .ok_or("missing incident property handle")?;
    let mut report = backend.run(&CheckPlan {
        invariant: invariant.id.clone(),
        model: MODEL.into(),
        handle: handle.clone(),
        bounds: [
            ("max_depth".into(), json!(steps.len() + 12)),
            ("max_states".into(), json!(20_000)),
            ("max_actions".into(), json!(32)),
        ]
        .into(),
        seed: None,
        timeout_ms: NonZeroU64::new(10_000).expect("constant nonzero"),
    });
    crate::checking::qualify_projection(&mut report, &crate::embedded_registry()?, deadline);
    Ok(report)
}

pub(crate) fn run(
    args: impl Iterator<Item = OsString>,
    explore_prefix: bool,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let args: Vec<_> = args.collect();
    if args.len() != 2 {
        let _ = writeln!(
            errors,
            "usage: labby-verify incident[-explore] <lifecycle-json> <invariant>"
        );
        return 2;
    }
    let result = crate::read_bounded_regular(Path::new(&args[0]), 1_048_576)
        .map_err(|_| "cannot read bounded incident input".to_string())
        .and_then(|input| {
            reduce(
                &input,
                args[1].to_str().ok_or("invalid invariant encoding")?,
            )
        });
    match result {
        Ok(result) => {
            let (value, status) = if explore_prefix {
                match explore(&result) {
                    Ok(report) => {
                        let status = i32::from(!matches!(
                            report.verdict,
                            verify_core::Verdict::Bounded { .. }
                        ));
                        (json!({"incident":result,"exploration":report}), status)
                    }
                    Err(_) => {
                        let _ = writeln!(errors, "incident exploration failed");
                        return 2;
                    }
                }
            } else {
                (
                    serde_json::to_value(&result).expect("serializable reduced incident"),
                    0,
                )
            };
            if serde_json::to_writer(&mut *output, &value).is_err() || writeln!(output).is_err() {
                return 2;
            }
            status
        }
        Err(reason) => {
            let _ = writeln!(errors, "incident reduction failed: {reason}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incident_redaction_and_unreproduced_status_are_literal() {
        let input = json!({"schema":1,"authorization":"secret-canary","events":[
            {"step":{"action":"connect","generation":"secret-canary"},"raw":"secret-canary"},
            {"step":{"action":"admit","request":"private-person"}},
            {"step":{"action":"dispatch","request":"private-person"}},
            {"step":{"action":"cancel","request":"private-person"}},
            {"step":{"action":"complete_success","request":"private-person","generation":"secret-canary"}}
        ]});
        let result = reduce(&input.to_string(), "LABBY-REQ-003").unwrap();
        let rendered = serde_json::to_string(&result).unwrap();
        assert!(!rendered.contains("secret-canary"));
        assert!(!rendered.contains("private-person"));
        assert_eq!(result.scenario.status, Status::Unreproduced);
        assert_eq!(result.replay.verdict, TraceVerdict::InvariantHolds);
        assert!(!result.replay.gate_failure);
        let exploration = explore(&result).unwrap();
        assert!(matches!(
            exploration.verdict,
            verify_core::Verdict::Bounded { .. }
        ));
        assert!(exploration.scenarios.is_empty());
        assert_eq!(
            result.scenario.steps[0],
            json!({"action":"connect","generation":"generation_0"})
        );
    }
    #[test]
    fn malformed_payload_diagnostics_cannot_echo_secrets() {
        let error = reduce(
            r#"{"schema":1,"events":[{"step":{"action":"secret-canary"}}]}"#,
            "LABBY-REQ-001",
        )
        .err()
        .unwrap();
        assert_eq!(error, "invalid structured lifecycle incident");
        assert!(reduce(r#"{"schema":1,"events":[]}"#, "LABBY-REQ-001").is_err());
    }

    #[test]
    fn reduction_rejects_invalid_bounds_and_invariant_without_payload_echo() {
        let event = json!({"step":{"action":"cancel","request":"private"}});
        for input in [
            json!({"schema":2,"events":[event.clone()]}).to_string(),
            json!({"schema":1,"events":vec![event.clone();257]}).to_string(),
            "x".repeat(1_048_577),
        ] {
            assert!(reduce(&input, "LABBY-REQ-001").is_err());
        }
        let input = json!({"schema":1,"events":[event]}).to_string();
        assert_eq!(
            reduce(&input, "LABBY-REQ-999").err().unwrap(),
            "unknown lifecycle invariant"
        );
        assert_eq!(
            reduce(&input, "secret bearer value").err().unwrap(),
            "invalid invariant identity"
        );
    }

    #[test]
    fn source_provenance_changes_without_changing_canonical_scenario_identity() {
        let first = reduce(
            r#"{"schema":1,"events":[{"step":{"action":"connect","generation":"one"}}]}"#,
            "LABBY-REQ-001",
        )
        .unwrap();
        let second = reduce(
            r#"{"schema":1,"events":[{"step":{"action":"connect","generation":"two"},"extra":"discarded"}]}"#,
            "LABBY-REQ-001",
        )
        .unwrap();
        assert_eq!(first.scenario.fingerprint, second.scenario.fingerprint);
        assert_ne!(
            first.scenario.origin.reference,
            second.scenario.origin.reference
        );
        assert_eq!(first.scenario.steps, second.scenario.steps);
    }

    #[test]
    fn public_exploration_rejects_mutated_identity_and_unredacted_steps() {
        let input = r#"{"schema":1,"events":[{"step":{"action":"connect","generation":"one"}}]}"#;
        let mut reduced = reduce(input, "LABBY-REQ-001").unwrap();
        reduced.scenario.project = "different-project".into();
        reduced.scenario.fingerprint = None;
        assert!(explore(&reduced).is_err());
        let mut reduced = reduce(input, "LABBY-REQ-001").unwrap();
        reduced.scenario.steps[0] = json!({"action":"connect","generation":"private"});
        reduced.scenario.fingerprint = None;
        assert_eq!(
            explore(&reduced).err().unwrap(),
            "incident exploration requires canonical identifiers"
        );
    }

    #[test]
    fn structured_logs_select_one_lineage_and_preserve_observed_transitions() {
        let input = json!({"schema":1,"generation":"selected-secret","events":[
            {"fields":{"action":"unrelated","authorization":"secret-canary"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"other"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected-secret"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"selected-secret","call_id":"private-call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"cancelled","generation_id":"selected-secret","call_id":"private-call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"replaced","generation_id":"new-secret","previous_generation_id":"selected-secret"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"new-secret","call_id":"second-call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"dispatched","generation_id":"new-secret","call_id":"second-call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"succeeded","generation_id":"new-secret","call_id":"second-call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"disconnected","generation_id":"new-secret"}}
        ]});
        let reduced = reduce(&input.to_string(), "LABBY-REQ-005").unwrap();
        assert_eq!(
            reduced.scenario.steps,
            vec![
                json!({"action":"connect","generation":"generation_0"}),
                json!({"action":"admit","request":"request_0"}),
                json!({"action":"cancel","request":"request_0"}),
                json!({"action":"replace_connection","generation":"generation_1"}),
                json!({"action":"admit","request":"request_1"}),
                json!({"action":"dispatch","request":"request_1"}),
                json!({"action":"complete_success","request":"request_1","generation":"generation_1"}),
                json!({"action":"disconnect","generation":"generation_1"}),
            ]
        );
        let rendered = serde_json::to_string(&reduced).unwrap();
        for secret in [
            "selected-secret",
            "new-secret",
            "private-call",
            "secret-canary",
        ] {
            assert!(!rendered.contains(secret));
        }
        assert_eq!(reduced.replay.verdict, TraceVerdict::InvariantHolds);
        assert_eq!(reduced.scenario.status, Status::Unreproduced);
    }

    #[test]
    fn canonicalization_cannot_turn_invalid_identifiers_into_applied_steps() {
        let input = r#"{"schema":1,"events":[{"step":{"action":"connect","generation":""}}]}"#;
        let reduced = reduce(input, "LABBY-REQ-001").unwrap();
        assert_eq!(
            reduced.scenario.steps[0],
            json!({"action":"connect","generation":""})
        );
        assert!(matches!(
            reduced.replay.observations[1].outcome,
            Some(verify_core::StepOutcome::Rejected { .. })
        ));
    }

    #[test]
    fn structured_logs_do_not_invent_missing_connection_or_dispatch() {
        let input = json!({"schema":1,"generation":"selected","events":[
            {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"selected","call_id":"call"}}
        ]});
        assert_eq!(
            reduce(&input.to_string(), "LABBY-REQ-005").err().unwrap(),
            "incident lacks the selected connection start"
        );
        let input = json!({"schema":1,"generation":"selected","events":[
            {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"selected","call_id":"call"}},
            {"fields":{"action":"browser.call.lifecycle","phase":"cancelled","generation_id":"selected","call_id":"call"}}
        ]});
        let reduced = reduce(&input.to_string(), "LABBY-REQ-005").unwrap();
        assert_eq!(reduced.scenario.steps.len(), 3);
        assert_eq!(
            reduced.scenario.steps[2],
            json!({"action":"cancel","request":"request_0"})
        );
    }

    #[test]
    fn structured_logs_require_every_transition_to_apply() {
        for events in [
            json!([
                {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"succeeded","generation_id":"selected","call_id":"call"}}
            ]),
            json!([
                {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"selected","call_id":"call"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"disconnected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"failed","generation_id":"selected","call_id":"call"}}
            ]),
            json!([
                {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"disconnected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"disconnected","generation_id":"selected"}}
            ]),
        ] {
            let input = json!({"schema":1,"generation":"selected","events":events});
            assert!(matches!(
                reduce(&input.to_string(), "LABBY-REQ-001"),
                Err(error) if error == "structured lifecycle transition is out of order"
            ));
        }
    }

    #[test]
    fn structured_pre_dispatch_failures_are_applied_without_inventing_dispatch() {
        for phase in [
            "persistence_failed",
            "revalidation_failed",
            "dispatch_failed",
        ] {
            let input = json!({"schema":1,"generation":"selected","events":[
                {"fields":{"action":"browser.call.lifecycle","phase":"connected","generation_id":"selected"}},
                {"fields":{"action":"browser.call.lifecycle","phase":"admitted","generation_id":"selected","call_id":"call"}},
                {"fields":{"action":"browser.call.lifecycle","phase":phase,"generation_id":"selected","call_id":"call"}}
            ]});
            let reduced = reduce(&input.to_string(), "LABBY-REQ-005").unwrap();
            assert_eq!(
                reduced.scenario.steps[2],
                json!({"action":"fail_before_dispatch","request":"request_0"})
            );
            assert_eq!(reduced.replay.verdict, TraceVerdict::InvariantHolds);
        }
    }
}
