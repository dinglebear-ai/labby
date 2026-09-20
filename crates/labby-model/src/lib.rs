//! Deterministic, transport-free models of selected Labby product behavior.
//!
//! Product code must never depend on this crate. It exists for scenario replay
//! and later model-checker adapters.

mod browser_request;

/// Descriptive alias for the request-lifecycle scenario step type.
pub use browser_request::Step as BrowserRequestStep;
pub use browser_request::{
    BrowserRequestModel, BrowserRequestState, RequestPhase, Step, TerminalOutcome,
};

/// Target registry key used by the invariant catalog and scenario corpus.
pub const MODEL: &str = "browser_request";

/// The request-lifecycle invariant catalog compiled into verification hosts.
pub const CATALOG_TOML: &str = include_str!("../../../tools/verification/formal/invariants.toml");

/// Parse and validate the built-in catalog against a caller-owned backend set.
pub fn catalog(
    backends: &verify_core::BackendRegistry<'_>,
) -> Result<verify_core::ValidatedCatalog, verify_core::CatalogError> {
    verify_core::Catalog::from_toml(CATALOG_TOML, backends)
}
