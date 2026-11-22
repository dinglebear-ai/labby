//! Shared security preflight guards used by the gateway runtime.
//!
//! Owned here (not in `labby-runtime`) because both modules are gateway-only:
//! neither `labby-auth` nor `labby-codemode` ever call into them.

pub mod guided_cli;
pub mod guided_openapi;
pub(crate) mod guided_stdio;
pub mod spawn_guard;
pub mod ssrf;
pub mod stdio_secret_env;
