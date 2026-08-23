mod bootstrap;
mod domain;
mod error;
mod health;
mod integrity;
mod migrations;
mod resolver;
mod store;

#[allow(unused_imports)]
pub(crate) use bootstrap::{BootstrapOutcome, BootstrapOwnerInput};
pub(crate) use health::{AccessHealth, AccessHealthStatus, inspect_health};
#[allow(unused_imports)]
pub(crate) use store::AccessStore;

#[cfg(test)]
mod facade_tests {
    #[test]
    fn bootstrap_facade_is_crate_private_callable() {
        fn accepts(_: super::BootstrapOwnerInput) {}
        fn returns(_: super::BootstrapOutcome) {}
        let _ = (accepts, returns);
    }
}
