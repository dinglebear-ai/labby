//! Native pairing policy. Transport keys never establish product authority.

pub(crate) mod assets;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) mod exchange;
mod pairing;
pub use pairing::{ApprovedPairing, PairingError, PairingRequest, PendingPairing};

mod grant;
pub(crate) mod listener;
#[cfg(all(feature = "tailcat", unix))]
mod session;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) use session::{HelperArtifact, SessionDelivery};
mod authorization;
pub(crate) mod cleanup;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) mod client;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) mod manager;
pub(crate) use authorization::RequestAuthority;
#[cfg(all(test, feature = "tailcat", unix))]
mod acceptance;
#[cfg(all(test, unix))]
pub(crate) mod testing;
