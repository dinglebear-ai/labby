//! Stable surface contract for the single `skills` service.

#![allow(
    dead_code,
    reason = "surface adapters consume this Wave 3 contract incrementally"
)]

pub(crate) const SERVICE_NAME: &str = "skills";
pub(crate) const FEATURE_GATE: &str = "skills";
pub(crate) const CLI_COMMAND: &str = "skills";
pub(crate) const HTTP_ROUTE: &str = "/v1/skills";
pub(crate) const MCP_TOOL_PERMANENT: bool = true;
const _: () = assert!(MCP_TOOL_PERMANENT);
pub(crate) const READ_SCOPES: &[&str] = &["lab:read", "lab", "lab:admin"];
pub(crate) const MUTATION_SCOPES: &[&str] = &["lab", "lab:admin"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_contract_keeps_one_service_and_existing_routes() {
        assert_eq!(
            (SERVICE_NAME, FEATURE_GATE, CLI_COMMAND, HTTP_ROUTE),
            ("skills", "skills", "skills", "/v1/skills")
        );
        assert_eq!(READ_SCOPES, ["lab:read", "lab", "lab:admin"]);
        assert_eq!(MUTATION_SCOPES, ["lab", "lab:admin"]);
    }
}
