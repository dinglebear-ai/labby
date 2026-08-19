//! Surface-neutral Artifact domain and local runtime support.
//!
//! This module is the open personal Artifact foundation for Labby. It owns the
//! portable ArtifactInterchange v1 contract, deterministic content addressing,
//! validation, Agent Skills projection, and the local immutable-revision store.
//! Product transports remain adapters over this layer.

pub mod canonical_json;
mod local_io;
pub mod model;
pub mod skill;
pub mod store;
mod store_ops;
pub mod validation;

pub use model::{
    ARTIFACT_INTERCHANGE_SCHEMA, ArtifactComponent, ArtifactDescriptor, ArtifactInterchange,
    ArtifactLicenseState, ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRecord,
    ArtifactRevision, Distribution, ExecutionRisk, JsonMap, PublicationState, Redistribution,
    ReviewState, TakedownState, Visibility,
};
pub use store::{ArtifactExportOptions, ArtifactForkRequest, ArtifactImportRequest, ArtifactStore};

use thiserror::Error;

/// Stable errors produced by the surface-neutral Artifact implementation.
///
/// Errors deliberately avoid embedding source bytes, credentials, or arbitrary
/// metadata values so callers can safely project them to CLI, API, or MCP.
#[derive(Debug, Error)]
pub enum ArtifactError {
    /// A field failed a bounded contract check.
    #[error("artifact field `{field}` is invalid: {reason}")]
    InvalidField {
        /// Stable field label.
        field: &'static str,
        /// Stable, non-secret reason code.
        reason: &'static str,
    },
    /// A portable schema version is unsupported.
    #[error("unsupported Artifact schema version")]
    UnsupportedSchema,
    /// A logical path failed containment or normalization rules.
    #[error("artifact path is unsafe: {0}")]
    UnsafePath(&'static str),
    /// An operation exceeded a documented safety budget.
    #[error("artifact {what} exceeds limit {limit}")]
    LimitExceeded {
        /// Stable budget label.
        what: &'static str,
        /// Maximum accepted value.
        limit: u64,
    },
    /// A record or revision could not be found.
    #[error("artifact {0} was not found")]
    NotFound(&'static str),
    /// Existing immutable state disagreed with the requested write.
    #[error("artifact conflict: {0}")]
    Conflict(&'static str),
    /// Another process currently holds the artifact mutation lock.
    #[error("artifact is busy")]
    Busy,
    /// Safe-by-default export found content that resembles credential material.
    #[error("artifact export blocked because secret-like material was detected in `{path}`")]
    SecretMaterialDetected {
        /// Relative package path only. Never file contents.
        path: String,
    },
    /// Existing Agent Skills verification rejected a projected resource.
    #[error("Agent Skill resource verification failed")]
    SkillVerification,
    /// Local filesystem operation failed.
    #[error("artifact I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or parsing failed.
    #[error("artifact JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub(crate) fn invalid(field: &'static str, reason: &'static str) -> ArtifactError {
    ArtifactError::InvalidField { field, reason }
}
