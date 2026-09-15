//! Shared Q2 OAuth qualification helpers built on the canonical live harness.

#[path = "oauth_qualification/authelia_fixture.rs"]
mod authelia_fixture;
#[path = "oauth_qualification/evidence.rs"]
mod evidence;
#[cfg(feature = "proxy-testkit")]
#[path = "oauth_qualification/google_fixture.rs"]
mod google_fixture;

pub(crate) use authelia_fixture::*;
pub(crate) use evidence::*;
#[cfg(feature = "proxy-testkit")]
pub(crate) use google_fixture::*;
