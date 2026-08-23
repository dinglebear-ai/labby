mod bootstrap;
mod domain;
mod error;
mod health;
mod integrity;
mod migrations;
mod resolver;
mod store;
mod workflow;

#[allow(unused_imports)]
pub(crate) use bootstrap::{BootstrapOutcome, BootstrapOwnerInput};
pub(crate) use health::{AccessHealth, AccessHealthStatus, inspect_health};
#[allow(unused_imports)]
pub(crate) use store::AccessStore;
#[allow(unused_imports)]
pub(crate) use workflow::{OwnerBootstrapError, bootstrap_owner_at};

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
