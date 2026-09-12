//! The object-safe bridge that makes a heterogeneous target registry possible.
//!
//! [`ScenarioTarget`] has associated types, so it is not object-safe and a
//! registry cannot hold `Box<dyn ScenarioTarget>`. [`DynTarget`] is the erased
//! form, phrased entirely in `serde_json::Value`, and the blanket impl below
//! does the deserialize-and-erase work once.
//!
//! Adopting projects implement `ScenarioTarget` and never see this module. It
//! is `pub` only because the registry's type signature mentions it.

use serde_json::Value;
use verify_core::target::{ScenarioError, ScenarioTarget, StepOutcome};
use verify_core::{InvariantId, InvariantResult};

/// Erased state, opaque to everything but the target that produced it.
pub struct ErasedState(Box<dyn std::any::Any>);

/// The object-safe form of [`ScenarioTarget`].
pub trait DynTarget {
    fn init_erased(&self, initial: &Value) -> Result<ErasedState, ScenarioError>;

    fn apply_erased(
        &self,
        state: &mut ErasedState,
        step: &Value,
    ) -> Result<StepOutcome, ScenarioError>;

    fn check_erased(
        &self,
        invariant: &InvariantId,
        state: &ErasedState,
    ) -> Result<InvariantResult, ScenarioError>;

    fn commutes_erased(&self, a: &Value, b: &Value) -> bool;

    fn allows_identifier_renaming(&self) -> bool {
        false
    }
}

impl<T> DynTarget for T
where
    T: ScenarioTarget,
    T::State: 'static,
{
    fn init_erased(&self, initial: &Value) -> Result<ErasedState, ScenarioError> {
        let state = self.init(initial)?;
        Ok(ErasedState(Box::new(state)))
    }

    fn apply_erased(
        &self,
        state: &mut ErasedState,
        step: &Value,
    ) -> Result<StepOutcome, ScenarioError> {
        let step: T::Step = serde_json::from_value(step.clone())
            .map_err(|source| ScenarioError::step(source.to_string()))?;
        let typed = downcast_mut::<T>(state)?;
        self.apply(typed, &step)
    }

    fn check_erased(
        &self,
        invariant: &InvariantId,
        state: &ErasedState,
    ) -> Result<InvariantResult, ScenarioError> {
        let typed = downcast_ref::<T>(state)?;
        self.check(invariant, typed)
    }

    fn allows_identifier_renaming(&self) -> bool {
        ScenarioTarget::allows_identifier_renaming(self)
    }

    fn commutes_erased(&self, a: &Value, b: &Value) -> bool {
        // A step that cannot be deserialized cannot be known to commute. Saying
        // "no" is always sound; guessing "yes" would silently merge distinct
        // counterexamples.
        let (Ok(a), Ok(b)) = (
            serde_json::from_value::<T::Step>(a.clone()),
            serde_json::from_value::<T::Step>(b.clone()),
        ) else {
            return false;
        };
        self.commutes(&a, &b)
    }
}

/// A state that belongs to a different target than the one asked to use it.
///
/// Only reachable by registering one target and replaying with another, which
/// the registry prevents — but erasing a type and hoping is exactly how that
/// stops being true, so it is an error rather than a panic.
fn downcast_ref<T>(state: &ErasedState) -> Result<&T::State, ScenarioError>
where
    T: ScenarioTarget,
    T::State: 'static,
{
    state
        .0
        .downcast_ref::<T::State>()
        .ok_or_else(|| ScenarioError::initial("state belongs to a different target"))
}

fn downcast_mut<T>(state: &mut ErasedState) -> Result<&mut T::State, ScenarioError>
where
    T: ScenarioTarget,
    T::State: 'static,
{
    state
        .0
        .downcast_mut::<T::State>()
        .ok_or_else(|| ScenarioError::initial("state belongs to a different target"))
}
