//! Restricted native Tailcat transport lifecycle for Labby.

mod config;
mod error;
mod protocol;
#[cfg(unix)]
mod supervisor;

pub use config::{BridgeConfig, ValidatedBridgeConfig};
pub use error::BridgeError;
#[cfg(unix)]
pub use supervisor::{Bridge, BridgeStatus, ConnectionCapability};
