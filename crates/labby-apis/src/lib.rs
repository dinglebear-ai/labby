//! Pure SDK contracts used by the Labby MCP gateway.
//!
//! Provides the cross-cutting SDK primitives and the supported doctor and setup
//! contracts used by the Labby gateway product.

#![cfg_attr(docsrs, feature(doc_cfg))]

/// Cross-cutting primitives: HTTP client, auth, errors, status, action specs.
pub mod core;

/// Doctor — bootstrap health audit: env vars, system probes, service reachability.
pub mod doctor;

/// Setup — first-run + draft-commit configuration flow (Bootstrap orchestrator).
pub mod setup;

/// Provider-neutral artifact control-plane client contracts.
pub mod artifact_control;
