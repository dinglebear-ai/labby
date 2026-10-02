//! Native pairing policy. Transport keys never establish product authority.

mod pairing;
pub use pairing::{ApprovedPairing, PairingError, PairingRequest, PendingPairing};

mod grant;
#[cfg(all(feature = "tailcat", unix))]
mod session;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) use session::{HelperArtifact, SessionDelivery};
mod authorization;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) mod client;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) mod manager;
pub(crate) use authorization::RequestAuthority;
#[cfg(all(test, unix))]
pub(crate) mod testing;
