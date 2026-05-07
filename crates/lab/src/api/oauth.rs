//! Compatibility re-export shim — `AuthContext` now lives in the shared
//! `lab_auth` crate. Existing `use crate::api::oauth::AuthContext;` import
//! sites continue to compile via this re-export.
//!
//! `www_authenticate_value` likewise re-exported for the (rare) lab callers
//! that build their own `WWW-Authenticate` header outside of the auth layer.

pub use lab_auth::auth_context::AuthContext;
