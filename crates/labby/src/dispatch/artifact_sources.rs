//! One admission rule for host-configured Artifact sources.
//!
//! `[[artifacts.sources]]` is projected twice at startup: by
//! [`ArtifactControlPlane`](super::artifact_control::ArtifactControlPlane) for
//! curated operations and uploads, and by
//! [`ImportCoordinator`](super::skill_library::import::ImportCoordinator) for
//! exact acquisition. The two must reach the same verdict for every source, or
//! `artifacts.list` works while `artifacts.import` reports an unknown source.
//! This module owns the verdict: a source is admitted on every path, or it is
//! disabled on every path with one operator-visible reason. Neither consumer
//! may add a private variant of these checks.
//!
//! Checks run in one fixed order: id validity, duplicate ids (every copy is
//! disabled), the Public Depot acquisition binding, Public Depot credential
//! reuse, the endpoint URL, the control-plane origin, then `pinned_addresses`
//! against the endpoint host and, for control-plane sources, against the
//! control-plane host as well. Pins are judged by the Depot network policy in
//! [`super::depot::network`], which refuses IPv4-mapped IPv6 and cloud metadata
//! addresses outright and admits a private address only through an exact
//! `[depot.private_hosts]` grant for that host.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;

#[cfg(test)]
use labby_runtime::artifacts::ArtifactError;

use crate::config::depot::{DepotPreferences, PUBLIC_ID};
use crate::config::{ArtifactPreferences, ArtifactSourceConfig, ArtifactSourceKind};
use crate::dispatch::depot::network::{NetworkPolicy, validate_addresses};
#[cfg(test)]
use crate::dispatch::error::ToolError;

/// A source every path may use.
pub(crate) struct AdmittedArtifactSource<'a> {
    pub(crate) source: &'a ArtifactSourceConfig,
    /// Parsed exact-acquisition endpoint.
    pub(crate) endpoint: url::Url,
    /// Exact private grants for the endpoint host; the acquisition transport
    /// re-checks its pins against these on every request.
    pub(crate) endpoint_private_grants: BTreeSet<IpAddr>,
}

/// A source no path may use, with the one reason the operator sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RejectedArtifactSource {
    pub(crate) id: String,
    pub(crate) reason: SourceRejection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceRejection {
    HostPolicy,
    InvalidId,
    DuplicateId,
    PublicAcquisitionBinding,
    PublicCredentialReuse,
    InvalidEndpoint,
    ControlPlaneUnsupported,
    ControlPlaneOrigin,
    ControlPlanePath,
    PinnedAddresses { role: &'static str, host: String },
}

impl SourceRejection {
    /// The configuration parameter the operator has to fix.
    #[cfg(test)]
    fn param(&self) -> &'static str {
        match self {
            Self::HostPolicy => "depot.private_hosts",
            Self::InvalidId | Self::DuplicateId => "id",
            Self::PublicAcquisitionBinding => "depot.public_read_binding",
            Self::PublicCredentialReuse => "bearer_token_env",
            Self::InvalidEndpoint => "endpoint",
            Self::ControlPlaneUnsupported | Self::ControlPlaneOrigin | Self::ControlPlanePath => {
                "control_plane_url"
            }
            Self::PinnedAddresses { .. } => "pinned_addresses",
        }
    }
}

impl fmt::Display for SourceRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostPolicy => f.write_str(
                "[depot.private_hosts] is invalid, so every remote Artifact source is disabled",
            ),
            Self::InvalidId => f.write_str("connection id is not a valid identifier"),
            Self::DuplicateId => f.write_str("connection id is configured more than once"),
            Self::PublicAcquisitionBinding => {
                f.write_str("Public Depot acquisition binding is invalid")
            }
            Self::PublicCredentialReuse => {
                f.write_str("the Public Depot credential cannot authorize another source")
            }
            Self::InvalidEndpoint => f.write_str("endpoint is not a URL with a host"),
            Self::ControlPlaneUnsupported => {
                f.write_str("control_plane_url is supported only for Depot sources")
            }
            Self::ControlPlaneOrigin => {
                f.write_str("control_plane_url must be a public HTTPS origin")
            }
            Self::ControlPlanePath => f.write_str("control_plane_url must not include a path"),
            Self::PinnedAddresses { role, host } => write!(
                f,
                "pinned_addresses are not authorized by [depot.private_hosts] for the {role} host `{host}`"
            ),
        }
    }
}

/// Strict test constructors surface the first rejection as the error type of
/// the path under test, so unit tests can assert the exact verdict.
#[cfg(test)]
impl RejectedArtifactSource {
    /// The strict control-plane constructor's error for this rejection.
    pub(crate) fn to_tool_error(&self) -> ToolError {
        match &self.reason {
            SourceRejection::DuplicateId => ToolError::Conflict {
                message: "Duplicate Artifact source connection".to_owned(),
                existing_id: self.id.clone(),
            },
            reason => ToolError::InvalidParam {
                message: format!("Artifact source `{}` is disabled: {reason}", self.id),
                param: reason.param().to_owned(),
            },
        }
    }

    /// The strict import constructor's error for this rejection.
    pub(crate) fn to_artifact_error(&self) -> ArtifactError {
        match &self.reason {
            SourceRejection::HostPolicy => ArtifactError::Conflict("invalid_host_policy"),
            SourceRejection::InvalidId => ArtifactError::InvalidField {
                field: "connection_id",
                reason: "invalid_id",
            },
            SourceRejection::DuplicateId => {
                ArtifactError::Conflict("duplicate_import_connection_id")
            }
            SourceRejection::PublicAcquisitionBinding => {
                ArtifactError::Conflict("public_acquisition_binding_invalid")
            }
            SourceRejection::PublicCredentialReuse => {
                ArtifactError::Conflict("public_credential_reuse")
            }
            SourceRejection::InvalidEndpoint => ArtifactError::InvalidField {
                field: "source.endpoint",
                reason: "invalid_url",
            },
            SourceRejection::ControlPlaneUnsupported
            | SourceRejection::ControlPlaneOrigin
            | SourceRejection::ControlPlanePath => ArtifactError::InvalidField {
                field: "source.control_plane_url",
                reason: "invalid_control_plane",
            },
            SourceRejection::PinnedAddresses { .. } => {
                ArtifactError::UnsafePath("provider_dns_address")
            }
        }
    }
}

/// The admission verdict for one `[[artifacts.sources]]` list.
pub(crate) struct HostArtifactSources<'a> {
    pub(crate) admitted: Vec<AdmittedArtifactSource<'a>>,
    pub(crate) rejected: Vec<RejectedArtifactSource>,
    /// Host network policy shared by every admitted source; `None` when
    /// `[depot.private_hosts]` is invalid and nothing was admitted.
    pub(crate) policy: Option<NetworkPolicy>,
}

impl HostArtifactSources<'_> {
    /// Emit the one operator-visible warning per disabled source. Call it once
    /// per process startup, not once per consumer.
    pub(crate) fn warn_rejections(&self) {
        for rejected in &self.rejected {
            tracing::warn!(
                connection_id = %rejected.id,
                reason = %rejected.reason,
                "Artifact source disabled on every path"
            );
        }
    }
}

/// Admit the host's sources under its `[depot.private_hosts]` policy.
pub(crate) fn admit_host_sources<'a>(
    artifacts: &'a ArtifactPreferences,
    depot: &DepotPreferences,
) -> HostArtifactSources<'a> {
    match super::depot::manager::host_policy(depot) {
        Ok(policy) => admit_sources(artifacts, depot, policy),
        Err(_) => {
            let mut rejected = Vec::new();
            for source in &artifacts.sources {
                push_rejection(&mut rejected, &source.id, SourceRejection::HostPolicy);
            }
            HostArtifactSources {
                admitted: Vec::new(),
                rejected,
                policy: None,
            }
        }
    }
}

/// Admit sources under an explicit network policy.
pub(crate) fn admit_sources<'a>(
    artifacts: &'a ArtifactPreferences,
    depot: &DepotPreferences,
    policy: NetworkPolicy,
) -> HostArtifactSources<'a> {
    let mut copies: BTreeMap<&str, usize> = BTreeMap::new();
    for source in &artifacts.sources {
        *copies.entry(source.id.as_str()).or_default() += 1;
    }
    let mut admitted = Vec::new();
    let mut rejected = Vec::new();
    for source in &artifacts.sources {
        let copies = copies.get(source.id.as_str()).copied().unwrap_or(1);
        match admit_one(source, artifacts, depot, &policy, copies) {
            Ok(source) => admitted.push(source),
            Err(reason) => push_rejection(&mut rejected, &source.id, reason),
        }
    }
    HostArtifactSources {
        admitted,
        rejected,
        policy: Some(policy),
    }
}

fn push_rejection(rejected: &mut Vec<RejectedArtifactSource>, id: &str, reason: SourceRejection) {
    if rejected
        .iter()
        .any(|existing| existing.id == id && existing.reason == reason)
    {
        return;
    }
    rejected.push(RejectedArtifactSource {
        id: id.to_owned(),
        reason,
    });
}

fn admit_one<'a>(
    source: &'a ArtifactSourceConfig,
    artifacts: &ArtifactPreferences,
    depot: &DepotPreferences,
    policy: &NetworkPolicy,
    copies: usize,
) -> Result<AdmittedArtifactSource<'a>, SourceRejection> {
    labby_runtime::artifacts::validation::validate_id(&source.id, "connection_id")
        .map_err(|_| SourceRejection::InvalidId)?;
    if copies > 1 {
        return Err(SourceRejection::DuplicateId);
    }
    if source.id == PUBLIC_ID && depot.validate_public_acquisition(artifacts).is_err() {
        return Err(SourceRejection::PublicAcquisitionBinding);
    }
    if source.id != PUBLIC_ID
        && depot.public_read_binding.as_ref().is_some_and(|binding| {
            source.bearer_token_env.as_deref() == Some(&binding.bearer_token_env)
        })
    {
        return Err(SourceRejection::PublicCredentialReuse);
    }
    let endpoint =
        url::Url::parse(&source.endpoint).map_err(|_| SourceRejection::InvalidEndpoint)?;
    let endpoint_host = endpoint
        .host_str()
        .ok_or(SourceRejection::InvalidEndpoint)?
        .to_owned();
    let control_plane_host = match source.control_plane_url.as_deref() {
        None => None,
        Some(url) => {
            if source.kind == ArtifactSourceKind::Repository {
                return Err(SourceRejection::ControlPlaneUnsupported);
            }
            let parsed = labby_primitives::ssrf::parse_validated_https_url(url)
                .map_err(|_| SourceRejection::ControlPlaneOrigin)?;
            if parsed.path() != "/" {
                return Err(SourceRejection::ControlPlanePath);
            }
            Some(
                parsed
                    .host_str()
                    .ok_or(SourceRejection::ControlPlaneOrigin)?
                    .to_owned(),
            )
        }
    };
    validate_addresses(&endpoint_host, &source.pinned_addresses, policy).map_err(|_| {
        SourceRejection::PinnedAddresses {
            role: "endpoint",
            host: endpoint_host.clone(),
        }
    })?;
    if let Some(host) = control_plane_host {
        validate_addresses(&host, &source.pinned_addresses, policy).map_err(|_| {
            SourceRejection::PinnedAddresses {
                role: "control-plane",
                host,
            }
        })?;
    }
    let endpoint_private_grants = policy
        .private_hosts
        .get(&endpoint_host)
        .cloned()
        .unwrap_or_default();
    Ok(AdmittedArtifactSource {
        source,
        endpoint,
        endpoint_private_grants,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depot_source(
        id: &str,
        endpoint: &str,
        control_plane: Option<&str>,
        pin: &str,
    ) -> ArtifactSourceConfig {
        ArtifactSourceConfig {
            id: id.to_owned(),
            kind: ArtifactSourceKind::Depot,
            endpoint: endpoint.to_owned(),
            control_plane_url: control_plane.map(str::to_owned),
            pinned_addresses: vec![pin.parse().unwrap()],
            bearer_token_env: None,
        }
    }

    fn grants(entries: &[(&str, &str)]) -> NetworkPolicy {
        NetworkPolicy {
            private_hosts: entries
                .iter()
                .map(|(host, pin)| ((*host).to_owned(), BTreeSet::from([pin.parse().unwrap()])))
                .collect(),
            ..Default::default()
        }
    }

    fn ids<'a>(sources: &'a HostArtifactSources<'_>) -> Vec<&'a str> {
        sources
            .admitted
            .iter()
            .map(|admitted| admitted.source.id.as_str())
            .collect()
    }

    #[test]
    fn control_plane_sources_need_the_grant_for_both_hosts() {
        let artifacts = ArtifactPreferences {
            sources: vec![depot_source(
                "private",
                "https://depot.example.com/api/artifacts/exact",
                Some("https://cp.example.com"),
                "10.1.0.8",
            )],
        };
        let depot = DepotPreferences::default();
        let one_host = admit_sources(
            &artifacts,
            &depot,
            grants(&[("cp.example.com", "10.1.0.8")]),
        );
        assert!(ids(&one_host).is_empty());
        assert_eq!(
            one_host.rejected,
            [RejectedArtifactSource {
                id: "private".to_owned(),
                reason: SourceRejection::PinnedAddresses {
                    role: "endpoint",
                    host: "depot.example.com".to_owned(),
                },
            }]
        );
        let other_host = admit_sources(
            &artifacts,
            &depot,
            grants(&[("depot.example.com", "10.1.0.8")]),
        );
        assert_eq!(
            other_host.rejected[0].reason,
            SourceRejection::PinnedAddresses {
                role: "control-plane",
                host: "cp.example.com".to_owned(),
            }
        );
        let both = admit_sources(
            &artifacts,
            &depot,
            grants(&[
                ("cp.example.com", "10.1.0.8"),
                ("depot.example.com", "10.1.0.8"),
            ]),
        );
        assert_eq!(ids(&both), ["private"]);
        assert_eq!(
            both.admitted[0].endpoint_private_grants,
            BTreeSet::from(["10.1.0.8".parse::<IpAddr>().unwrap()])
        );
    }

    #[test]
    fn ipv4_mapped_public_pins_are_refused_like_every_other_mapped_form() {
        let artifacts = ArtifactPreferences {
            sources: vec![depot_source(
                "mapped",
                "https://depot.example.com/api/artifacts/exact",
                None,
                "::ffff:8.8.8.8",
            )],
        };
        let verdict = admit_sources(
            &artifacts,
            &DepotPreferences::default(),
            NetworkPolicy::default(),
        );
        assert!(ids(&verdict).is_empty());
        assert!(matches!(
            verdict.rejected[0].reason,
            SourceRejection::PinnedAddresses {
                role: "endpoint",
                ..
            }
        ));
    }

    #[test]
    fn duplicate_ids_disable_every_copy_with_one_warning() {
        let valid = depot_source(
            "x",
            "https://depot.example.com/api/artifacts/exact",
            None,
            "8.8.8.8",
        );
        let mut reuses_public_credential = valid.clone();
        reuses_public_credential.bearer_token_env =
            Some("LABBY_DEPOT_CATALOG_READ_TOKEN".to_owned());
        let artifacts = ArtifactPreferences {
            sources: vec![valid, reuses_public_credential],
        };
        let verdict = admit_sources(
            &artifacts,
            &DepotPreferences::default(),
            NetworkPolicy::default(),
        );
        assert!(ids(&verdict).is_empty());
        assert_eq!(
            verdict.rejected,
            [RejectedArtifactSource {
                id: "x".to_owned(),
                reason: SourceRejection::DuplicateId,
            }]
        );
    }

    #[test]
    fn invalid_host_policy_disables_every_source_without_admitting_any() {
        let mut depot = DepotPreferences::default();
        depot.extra.insert(
            "private_hosts".to_owned(),
            toml::Value::try_from(BTreeMap::from([("depot.internal", Vec::<String>::new())]))
                .unwrap(),
        );
        let artifacts = ArtifactPreferences {
            sources: vec![depot_source(
                "a",
                "https://depot.example.com/api/artifacts/exact",
                None,
                "8.8.8.8",
            )],
        };
        let verdict = admit_host_sources(&artifacts, &depot);
        assert!(verdict.policy.is_none());
        assert!(ids(&verdict).is_empty());
        assert_eq!(verdict.rejected[0].reason, SourceRejection::HostPolicy);
    }

    #[test]
    fn rejection_messages_name_the_parameter_to_fix() {
        let rejected = RejectedArtifactSource {
            id: "private".to_owned(),
            reason: SourceRejection::PinnedAddresses {
                role: "endpoint",
                host: "depot.example.com".to_owned(),
            },
        };
        let message = rejected.reason.to_string();
        assert!(message.contains("pinned_addresses"));
        assert!(message.contains("depot.example.com"));
        assert!(matches!(
            rejected.to_tool_error(),
            ToolError::InvalidParam { param, .. } if param == "pinned_addresses"
        ));
        assert!(matches!(
            rejected.to_artifact_error(),
            ArtifactError::UnsafePath("provider_dns_address")
        ));
    }
}
