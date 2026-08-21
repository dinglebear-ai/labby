//! Doctor service errors.

use crate::core::error::ApiError;

/// Errors produced by the doctor service SDK layer.
#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    /// Underlying shared API error.
    #[error(transparent)]
    Api(#[from] ApiError),
}
