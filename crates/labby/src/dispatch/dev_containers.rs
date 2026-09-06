//! Shared Dev Container dispatch orchestration.
//!
//! Transport adapters are intentionally absent. Runtime effects are delegated
//! to the surface-neutral, pluggable engine contract.

#[allow(unused_imports)]
pub(crate) use labby_runtime::dev_container_runtime::{
    ContainerRuntime, DurableIntent, EngineCreateRequest, EngineHandle, EngineState,
    RecoveryAction, RuntimeError, create, reconcile,
};
