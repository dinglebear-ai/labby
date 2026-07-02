//! OpenAPI-spec-to-Code-Mode-tool derivation. Parses specs via `rmcp-openapi`;
//! executes outbound HTTP via its OWN hardened `reqwest` client (redirects off,
//! peer-IP re-validated). Isolates `rmcp-openapi`/`reqwest` out of
//! `labby-codemode`. MUST NOT depend on `labby-codemode`/`labby-gateway`.
pub mod config;
pub mod error;
