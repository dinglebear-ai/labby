//! Deliberately tiny harness self-test; not a Labby model or conformance proof.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};
use verify_core::{
    BackendRegistry, Catalog, InvariantId, InvariantResult, ScenarioError, ScenarioTarget,
    StepOutcome, ValidatedCatalog,
};
use verify_runner::TargetRegistry;
use verify_scenario::{Scenario, ValidatedScenario};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub actor: String,
    pub value: i64,
}

#[derive(Default)]
pub struct Counter {
    pub broken_rename: bool,
    pub rename_length_change: i8,
    pub flaky: Option<Cell<bool>>,
}

impl ScenarioTarget for Counter {
    type State = i64;
    type Step = Step;
    fn init(&self, initial: &Value) -> Result<i64, ScenarioError> {
        match initial.get("value") {
            None => Ok(0),
            Some(value) => value
                .as_i64()
                .ok_or_else(|| ScenarioError::InvalidInitial("value must be integer".into())),
        }
    }
    fn apply(&self, state: &mut i64, step: &Step) -> Result<StepOutcome, ScenarioError> {
        if step.value == -99 {
            return Err(ScenarioError::Harness("fixture failure".into()));
        }
        if step.value == -1 {
            return Ok(StepOutcome::Rejected {
                reason: "fixture rejection".into(),
            });
        }
        *state = step.value;
        Ok(StepOutcome::Applied {})
    }
    fn check(&self, id: &InvariantId, state: &i64) -> Result<InvariantResult, ScenarioError> {
        if id.as_str() == "EX-COUNT-002" {
            return Err(ScenarioError::UnknownInvariant(id.clone()));
        }
        if *state == -2 {
            return Ok(InvariantResult::Incomplete {
                reason: "missing observation".into(),
            });
        }
        let violated = if let Some(flaky) = &self.flaky {
            let prior = flaky.get();
            flaky.set(!prior);
            prior
        } else {
            *state > 1
        };
        Ok(if violated {
            InvariantResult::Violated {
                reason: "counter exceeds one".into(),
            }
        } else {
            InvariantResult::Holds {}
        })
    }
    fn canonicalize(
        &self,
        initial: &Value,
        steps: &[Step],
    ) -> Result<(Value, Vec<Step>), ScenarioError> {
        let mut ids = BTreeMap::new();
        let mut steps: Vec<_> = steps
            .iter()
            .cloned()
            .map(|mut step| {
                let next = ids.len();
                let id = *ids.entry(step.actor.clone()).or_insert(next);
                step.actor = format!("actor_{id}");
                if self.broken_rename {
                    step.value = 0;
                }
                step
            })
            .collect();
        if self.rename_length_change < 0 {
            steps.clear();
        }
        if self.rename_length_change > 0
            && let Some(step) = steps.first().cloned()
        {
            steps.push(step);
        }
        Ok((initial.clone(), steps))
    }
}

pub fn catalog(kind: &str) -> ValidatedCatalog {
    Catalog::from_json(&json!({"schema":1,"project":"example","namespace":"EX","invariant":[
        {"id":"EX-COUNT-001","title":"Counter at most one","kind":kind,"severity":"high","model":"counter"},
        {"id":"EX-COUNT-002","title":"Unsupported fixture property","kind":kind,"severity":"high","model":"counter"}
    ]}).to_string(), &BackendRegistry::default()).unwrap()
}

pub fn registry(target: Counter, kind: &str) -> TargetRegistry<'static> {
    let mut registry = TargetRegistry::default();
    registry
        .register(&catalog(kind), "counter", target)
        .unwrap();
    registry
}

pub fn scenario(steps: &[i64], expect: &str, status: &str) -> ValidatedScenario {
    Scenario::from_json(&json!({"schema":1,"project":"example","model":"counter","invariant":"EX-COUNT-001",
        "origin":{"kind":"manual"},"steps":steps.iter().map(|value| json!({"actor":"upstream_7","value":value})).collect::<Vec<_>>(),
        "expect":expect,"status":status}).to_string()).unwrap()
}
