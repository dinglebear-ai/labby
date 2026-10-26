//! Optional Skills directory adapter. Namespace policy is conservative: only
//! routes with no configured, enabled proxied Skills namespace advertise it.

use crate::mcp::context::{auth_context_from_extensions, code_mode_read_scope_allowed};
use crate::mcp::server::LabMcpServer;
use crate::skills::{
    directory::{DirectoryParams, directory_page},
    facade::first_party_skill_listing,
};
use rmcp::{
    RoleServer,
    model::{CustomRequest, CustomResult, ErrorData},
    service::RequestContext,
};

#[cfg(any(feature = "gateway", test))]
fn native_namespaces_only<'a>(
    upstreams: impl IntoIterator<Item = (&'a str, bool, bool)>,
    allows: impl Fn(&str) -> bool,
) -> bool {
    !upstreams
        .into_iter()
        .any(|(name, enabled, proxy_skills)| enabled && proxy_skills && allows(name))
}

impl LabMcpServer {
    pub(crate) async fn supports_skill_directories(&self) -> bool {
        if !self.route_scope.exposes_skills() || !self.route_scope.exposes_resources() {
            return false;
        }
        #[cfg(feature = "gateway")]
        if let Some(manager) = &self.gateway_manager {
            let config = manager.current_config().await;
            return native_namespaces_only(
                config
                    .upstream
                    .iter()
                    .map(|u| (u.name.as_str(), u.enabled, u.proxy_skills)),
                |name| self.route_scope.allows_upstream(name),
            );
        }
        true
    }

    pub(crate) async fn handle_skill_directory_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                "reading skill directories requires lab:read",
                None,
            ));
        }
        // Recheck configuration on every request. An earlier discovery response
        // cannot authorize namespaces added by a later configuration revision.
        if !self.supports_skill_directories().await {
            return Err(ErrorData::invalid_params(
                "directory reading is unavailable for this route",
                None,
            ));
        }
        let params = request
            .params_as::<DirectoryParams>()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
            .ok_or_else(|| ErrorData::invalid_params("directory read requires uri", None))?;
        let registry = self
            .skill_registry_context(context)
            .await
            .map_err(super::skills::skill_read_error)?;
        let listing = first_party_skill_listing(&registry);
        let page = directory_page(&listing, &params, &registry.catalog_visibility_key())
            .map_err(super::skills::skill_read_error)?;
        let resources = page
            .resources
            .iter()
            .map(|child| {
                if let Some(skill) = listing.skills.iter().find(|skill| skill.uri == child.uri) {
                    return super::handlers_resources::skill_manifest_resource(skill);
                }
                let mut resource = rmcp::model::Resource::new(&child.uri, &child.name);
                resource.mime_type = child.mime_type.clone();
                resource
            })
            .collect::<Vec<_>>();
        let mut result = serde_json::to_value(page)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        result["resources"] = serde_json::to_value(resources)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        Ok(CustomResult::new(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_capability_tracks_configured_namespaces_and_route_exposure() {
        assert!(native_namespaces_only([], |_| true));
        assert!(native_namespaces_only([("remote", false, true)], |_| true));
        assert!(native_namespaces_only([("remote", true, false)], |_| true));
        assert!(native_namespaces_only([("remote", true, true)], |_| false));
        assert!(!native_namespaces_only([("remote", true, true)], |_| true));
        // The same configuration check is run again by the request handler,
        // so a newly enabled namespace revokes directory support immediately.
        let mut upstream = ("remote", false, true);
        assert!(native_namespaces_only([upstream], |_| true));
        upstream.1 = true;
        assert!(!native_namespaces_only([upstream], |_| true));
    }
}
