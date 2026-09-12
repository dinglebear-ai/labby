//! Where an adopting project declares which targets exist.
//!
//! Targets are keyed by `(project, model)`, taken from the scenario file, so a
//! scenario needs no path convention to find its target.

use std::collections::BTreeMap;

use crate::dyn_target::DynTarget;

/// Key identifying one model within one project.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TargetKey {
    pub project: String,
    pub model: String,
}

impl TargetKey {
    pub fn new(project: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            model: model.into(),
        }
    }
}

/// The set of targets this binary can replay against.
#[derive(Default)]
pub struct TargetRegistry {
    targets: BTreeMap<TargetKey, Box<dyn DynTarget>>,
}

impl TargetRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a target. Replacing an existing key returns the old target so a
    /// caller can notice rather than silently shadowing a registration.
    pub fn register(
        &mut self,
        key: TargetKey,
        target: Box<dyn DynTarget>,
    ) -> Option<Box<dyn DynTarget>> {
        self.targets.insert(key, target)
    }

    #[must_use]
    pub fn with(mut self, key: TargetKey, target: Box<dyn DynTarget>) -> Self {
        self.register(key, target);
        self
    }

    pub fn get(&self, key: &TargetKey) -> Option<&dyn DynTarget> {
        self.targets.get(key).map(AsRef::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Registered keys, for `verify targets` and for an error that can name
    /// what is available instead of only what is missing.
    pub fn keys(&self) -> impl Iterator<Item = &TargetKey> {
        self.targets.keys()
    }
}
