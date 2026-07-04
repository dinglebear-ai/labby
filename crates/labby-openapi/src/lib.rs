//! OpenAPI-spec-to-Code-Mode-tool derivation. Parses specs via `rmcp-openapi`;
//! executes outbound HTTP via its OWN hardened `reqwest` client (redirects off,
//! peer-IP re-validated). Isolates `rmcp-openapi`/`reqwest` out of
//! `labby-codemode`. MUST NOT depend on `labby-codemode`/`labby-gateway`.
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
pub use dispatch::dispatch_openapi_call;
pub use error::OpenApiError;
pub use registry::{OpenApiRegistry, OperationHandle, SpecEntry};

#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_dispatch;
#[cfg(test)]
mod tests_ssrf;
