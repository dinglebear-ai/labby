//! Allowlisted OpenAPI operation extraction and hardened outbound HTTP for
//! Code Mode. Extracts raw operation IDs, methods, and paths from JSON with
//! `serde_json`; loads specs and dispatches through guarded `reqwest` clients.
//! Hosts carry the registry and client through their contracts. Loading and
//! HTTP guards live here; this crate must not depend on `labby-codemode` or
//! `labby-gateway`.
pub mod config;
pub mod convert;
pub mod dispatch;
pub mod error;
pub mod http;
pub mod registry;
pub mod ssrf;

pub use config::{
    OpenApiCredential, OpenApiProviderConfig, OpenApiSpecConfig, RESERVED_NAMESPACES, SpecSource,
};
pub use dispatch::{dispatch_openapi_call, dispatch_openapi_call_with_credential};
pub use error::OpenApiError;
pub use registry::{OpenApiRegistry, OperationHandle, SpecEntry};

#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_dispatch;
#[cfg(test)]
mod tests_ssrf;
