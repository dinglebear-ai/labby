mod bootstrap;
mod domain;
mod error;
mod health;
mod integrity;
mod migrations;
mod read;
mod resolver;
mod runtime;
mod store;
mod workflow;

#[allow(unused_imports)]
pub(crate) use bootstrap::{BootstrapOutcome, BootstrapOwnerInput};
#[allow(unused_imports)]
pub(crate) use domain::ProjectRole;
pub(crate) use health::{AccessHealth, AccessHealthStatus, inspect_health};
#[allow(unused_imports)]
pub(crate) use read::{AccessibleProjectSnapshot, ProjectAccessSnapshot};
#[allow(unused_imports)]
pub(crate) use runtime::{
    AccessBlockedReason, AccessRuntime, AccessRuntimeError, AccessRuntimeStatus, AccessSetupReason,
};
#[allow(unused_imports)]
pub(crate) use store::AccessStore;
#[allow(unused_imports)]
pub(crate) use workflow::{OwnerBootstrapError, bootstrap_owner};

#[cfg(test)]
mod facade_tests {
    #[test]
    fn bootstrap_facade_is_crate_private_callable() {
        fn accepts(_: super::BootstrapOwnerInput) {}
        fn returns(_: super::BootstrapOutcome) {}
        fn workflow_errors(_: super::OwnerBootstrapError) {}
        let _ = (accepts, returns, workflow_errors);
    }
}
