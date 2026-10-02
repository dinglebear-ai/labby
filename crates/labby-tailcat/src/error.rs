use thiserror::Error;

/// Stable failures; never include executable paths, keys or raw helper output.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("invalid bridge configuration")]
    InvalidConfig,
    #[error("bridge artifact unavailable")]
    ArtifactUnavailable,
    #[error("bridge artifact checksum mismatch")]
    ChecksumMismatch,
    #[error("bridge process failed")]
    ProcessFailed,
    #[error("invalid bridge control protocol")]
    Protocol,
    #[error("bridge startup deadline exceeded")]
    StartupTimeout,
}
