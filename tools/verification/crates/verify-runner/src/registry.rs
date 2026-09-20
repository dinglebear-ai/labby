use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;
use verify_core::{InvariantId, Kind, ScenarioTarget, ValidatedCatalog};
use verify_scenario::ValidatedScenario;

use crate::{ReplayLimits, ReplayReport, TraceVerdict};

/// Invalid project-owned target registration.
#[derive(Debug, Error)]
pub enum RegistrationError {
    /// No property in the supplied catalog refers to this model.
    #[error("model is absent from catalog: {0}")]
    UnknownModel(String),
    /// Registration never silently replaces a target and its semantics.
    #[error("target already registered: {project}/{model}")]
    Duplicate {
        /// Adopting project.
        project: String,
        /// Model identity.
        model: String,
    },
}

/// Structured lookup failures retained through library APIs, before reporting.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TargetResolutionError {
    /// No target implementation was registered for this project/model pair.
    #[error("unknown target {project}/{model}; registered: {available:?}")]
    UnknownTarget {
        /// Requested project.
        project: String,
        /// Requested model.
        model: String,
        /// Registered alternatives in stable order.
        available: Vec<(String, String)>,
    },
    /// Target exists, but the catalog does not bind the requested property to it.
    #[error("invariant {invariant} is not registered for {project}/{model}")]
    UnknownInvariant {
        /// Requested project.
        project: String,
        /// Requested model.
        model: String,
        /// Requested property.
        invariant: InvariantId,
    },
}

pub(crate) trait ErasedTarget {
    fn replay(
        &self,
        scenario: &ValidatedScenario,
        kind: Kind,
        limits: &ReplayLimits,
    ) -> ReplayReport;
    fn canonicalize(
        &self,
        scenario: &ValidatedScenario,
    ) -> Result<(Map<String, Value>, Vec<Value>), String>;
}

struct Adapter<T>(T);
impl<T: ScenarioTarget> ErasedTarget for Adapter<T> {
    fn replay(
        &self,
        scenario: &ValidatedScenario,
        kind: Kind,
        limits: &ReplayLimits,
    ) -> ReplayReport {
        crate::replay::execute(&self.0, scenario, kind, limits)
    }
    fn canonicalize(
        &self,
        scenario: &ValidatedScenario,
    ) -> Result<(Map<String, Value>, Vec<Value>), String> {
        let raw = scenario.scenario();
        let steps: Vec<T::Step> = raw
            .steps
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let (initial, steps) = self
            .0
            .canonicalize(&Value::Object(raw.initial.clone()), &steps)
            .map_err(|e| e.to_string())?;
        if steps.len() != raw.steps.len() {
            return Err("identifier renaming must preserve the number of steps".into());
        }
        let Value::Object(initial) = initial else {
            return Err("canonical initial state is not an object".into());
        };
        let steps = steps
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        Ok((initial, steps))
    }
}

pub(crate) struct Entry<'a> {
    pub target: Box<dyn ErasedTarget + 'a>,
    pub invariants: BTreeMap<InvariantId, Kind>,
}

/// Caller-populated registry. Scenario data never loads code, executables or paths.
#[derive(Default)]
pub struct TargetRegistry<'a> {
    pub(crate) targets: BTreeMap<(String, String), Entry<'a>>,
}

impl<'a> TargetRegistry<'a> {
    /// Bind an implementation to a model and its catalogued property kinds.
    pub fn register<T: ScenarioTarget + 'a>(
        &mut self,
        catalog: &ValidatedCatalog,
        model: &str,
        target: T,
    ) -> Result<(), RegistrationError> {
        let key = (catalog.catalog().project.clone(), model.to_owned());
        if self.targets.contains_key(&key) {
            return Err(RegistrationError::Duplicate {
                project: key.0,
                model: key.1,
            });
        }
        let invariants: BTreeMap<_, _> = catalog
            .catalog()
            .invariant
            .iter()
            .filter(|invariant| invariant.model == model)
            .map(|invariant| (invariant.id.clone(), invariant.kind))
            .collect();
        if invariants.is_empty() {
            return Err(RegistrationError::UnknownModel(model.into()));
        }
        self.targets.insert(
            key,
            Entry {
                target: Box::new(Adapter(target)),
                invariants,
            },
        );
        Ok(())
    }

    /// Registered project/model pairs in stable order.
    pub fn targets(&self) -> Vec<(&str, &str)> {
        self.targets
            .keys()
            .map(|(project, model)| (project.as_str(), model.as_str()))
            .collect()
    }

    pub(crate) fn resolve(
        &self,
        scenario: &ValidatedScenario,
    ) -> Result<(&Entry<'a>, Kind), TargetResolutionError> {
        let raw = scenario.scenario();
        let entry = self
            .targets
            .get(&(raw.project.clone(), raw.model.clone()))
            .ok_or_else(|| TargetResolutionError::UnknownTarget {
                project: raw.project.clone(),
                model: raw.model.clone(),
                available: self.targets.keys().cloned().collect(),
            })?;
        let kind = entry.invariants.get(&raw.invariant).ok_or_else(|| {
            TargetResolutionError::UnknownInvariant {
                project: raw.project.clone(),
                model: raw.model.clone(),
                invariant: raw.invariant.clone(),
            }
        })?;
        Ok((entry, *kind))
    }

    /// Resolve catalog identity and replay, preserving typed trace outcomes.
    pub fn replay(&self, scenario: &ValidatedScenario, limits: &ReplayLimits) -> ReplayReport {
        match self.resolve(scenario) {
            Ok((entry, kind)) => entry.target.replay(scenario, kind, limits),
            Err(error) => ReplayReport::new(scenario, TraceVerdict::Error).fail(
                scenario,
                TraceVerdict::Error,
                error.to_string(),
            ),
        }
    }
}
