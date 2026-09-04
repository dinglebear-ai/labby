//! Stable surface contract for the Artifact management service.

#![allow(
    dead_code,
    reason = "kept as a compile-time assertion of the cross-surface service contract"
)]

pub(crate) const SERVICE_NAME: &str = "artifacts";
pub(crate) const FEATURE_GATE: &str = "skills";
pub(crate) const HTTP_ROUTE: &str = "/v1/artifacts";
pub(crate) const MCP_TOOL_PERMANENT: bool = true;
const _: () = assert!(MCP_TOOL_PERMANENT);
pub(crate) const READ_SCOPES: &[&str] = &["lab:read", "lab", "lab:admin"];
pub(crate) const MUTATION_SCOPES: &[&str] = &["lab", "lab:admin"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifact_management_contract_has_one_service_and_route() {
        assert_eq!(
            (SERVICE_NAME, FEATURE_GATE, HTTP_ROUTE),
            ("artifacts", "skills", "/v1/artifacts")
        );
        assert_eq!(READ_SCOPES, ["lab:read", "lab", "lab:admin"]);
        assert_eq!(MUTATION_SCOPES, ["lab", "lab:admin"]);
    }
}
