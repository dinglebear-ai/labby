//! Deterministic, transport-free models of selected Labby product behavior.
//!
//! Product code must never depend on this crate. It exists for scenario replay
//! and later model-checker adapters.

mod browser_request;
mod capability_visibility;

/// Descriptive alias for the request-lifecycle scenario step type.
pub use browser_request::Step as BrowserRequestStep;
pub use browser_request::{
    BrowserRequestModel, BrowserRequestState, RequestPhase, Step, TerminalOutcome,
};
pub use capability_visibility::{
    CapabilityCode, CapabilityVisibilityModel, CapabilityVisibilityState, Phase as CapabilityPhase,
    RuntimeCapability, Step as CapabilityVisibilityStep,
};

/// Browser request target registry key retained for existing incident tooling.
pub const MODEL: &str = "browser_request";
/// Capability-honesty target used for startup guard and degradation visibility.
pub const CAPABILITY_VISIBILITY_MODEL: &str = "capability_visibility";
/// All project-owned deterministic verification targets.
pub const MODELS: &[&str] = &[MODEL, CAPABILITY_VISIBILITY_MODEL];

/// The request-lifecycle invariant catalog compiled into verification hosts.
pub const CATALOG_TOML: &str = include_str!("../../../tools/verification/formal/invariants.toml");

/// Parse and validate the built-in catalog against a caller-owned backend set.
pub fn catalog(
    backends: &verify_core::BackendRegistry<'_>,
) -> Result<verify_core::ValidatedCatalog, verify_core::CatalogError> {
    verify_core::Catalog::from_toml(CATALOG_TOML, backends)
}
