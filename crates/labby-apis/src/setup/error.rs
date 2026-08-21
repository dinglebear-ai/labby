//! Typed errors for the `setup` service.

use thiserror::Error;

/// Library-side errors. The dispatch layer maps these into stable
/// envelope `kind` strings (see `crates/labby/src/dispatch/setup/`).
#[derive(Debug, Error)]
pub enum SetupError {
    /// A required setup parameter was omitted.
    #[error("missing required parameter: {0}")]
    MissingParam(String),

    /// A supplied setup value failed schema validation.
    #[error("invalid value for {field}: {reason}")]
    InvalidValue {
        /// Name of the invalid field.
        field: String,
        /// Human-readable validation failure.
        reason: String,
    },

    /// The requested setup service is not registered.
    #[error("unknown service: {0}")]
    UnknownService(String),
}
