//! Restricted native Tailcat transport lifecycle for Labby.

mod config;
mod error;
mod protocol;
#[cfg(unix)]
mod supervisor;

pub use config::{BridgeConfig, ValidatedBridgeConfig};
pub use error::BridgeError;
#[cfg(unix)]
pub use supervisor::{Bridge, BridgeStatus, ConnectionCapability, SealedDelivery};

/// Native transport availability is explicit, even when the lifecycle API is absent.
/// Product adapters must check this before presenting pairing actions.
pub fn ensure_platform_supported() -> Result<(), BridgeError> {
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        Ok(())
    } else {
        Err(BridgeError::UnsupportedPlatform)
    }
}
