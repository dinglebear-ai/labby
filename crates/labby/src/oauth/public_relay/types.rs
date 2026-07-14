use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MachineId(String);

impl MachineId {
    pub fn parse(value: &str) -> Result<Self, PublicRelayError> {
        let trimmed = value.trim();
        if trimmed != value || trimmed.is_empty() || trimmed.len() > 64 {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        // The alphanumeric/`-`/`_` charset also rejects `.`, so dot segments
        // (`.`, `..`) can never pass this check.
        if !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for MachineId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublicRelayError {
    #[error("invalid machine id")]
    InvalidMachineId(String),
    #[error("invalid callback suffix: {0}")]
    InvalidSuffix(String),
    #[error("invalid registry input: {0}")]
    InvalidRegistryInput(String),
    #[error("invalid callback request body: {0}")]
    InvalidRequestBody(String),
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("registry unavailable: {0}")]
    RegistryUnavailable(String),
    #[error("forwarder initialization failed: {0}")]
    ForwarderInitFailed(String),
    #[error("machine is not registered")]
    UnknownMachine,
    #[error("machine is disabled")]
    DisabledMachine,
    #[error("relay overloaded")]
    Overloaded,
    #[error("request body too large")]
    BodyTooLarge,
    #[error("upstream response too large")]
    ResponseTooLarge,
    #[error("upstream timeout")]
    UpstreamTimeout,
    #[error("upstream error")]
    UpstreamError,
}

impl PublicRelayError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidMachineId(_)
            | Self::InvalidSuffix(_)
            | Self::InvalidRegistryInput(_)
            | Self::InvalidRequestBody(_) => "invalid_param",
            Self::InvalidTarget(_) => "relay_invalid_target",
            Self::RegistryUnavailable(_) => "relay_registry_unavailable",
            Self::ForwarderInitFailed(_) => "relay_forwarder_init_failed",
            Self::UnknownMachine => "not_found",
            Self::DisabledMachine => "forbidden",
            Self::Overloaded => "queue_saturated",
            Self::BodyTooLarge => "content_too_large",
            Self::ResponseTooLarge => "content_too_large",
            Self::UpstreamTimeout => "timeout",
            Self::UpstreamError => "bad_gateway",
        }
    }

    pub fn to_tool_error(&self) -> ToolError {
        match self {
            Self::InvalidMachineId(_) => ToolError::InvalidParam {
                message: self.to_string(),
                param: "machine_id".to_string(),
            },
            Self::InvalidSuffix(_) => ToolError::InvalidParam {
                message: self.to_string(),
                param: "suffix".to_string(),
            },
            Self::InvalidRegistryInput(_) => ToolError::InvalidParam {
                message: self.to_string(),
                param: "registry".to_string(),
            },
            Self::InvalidRequestBody(_) => ToolError::InvalidParam {
                message: self.to_string(),
                param: "body".to_string(),
            },
            _ => ToolError::Sdk {
                sdk_kind: self.kind().to_string(),
                message: self.to_string(),
            },
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::Overloaded => "relay busy; retry later",
            // Shared by both the oversized-query-string and oversized-body
            // rejection paths (`api/services/oauth_relay.rs::callback`), so
            // the wording deliberately doesn't say "body" -- it isn't always
            // the body that was too large.
            Self::BodyTooLarge => "callback request too large (query or body)",
            Self::InvalidRequestBody(_) => "callback request invalid",
            Self::ResponseTooLarge => "callback response too large",
            Self::UpstreamTimeout => "callback target timed out",
            Self::UpstreamError => "callback target unavailable",
            _ => "callback target unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicRelayEntry {
    pub machine_id: MachineId,
    // Private: unvalidated raw input. Callers must go through `target_url()`
    // to read it and `PublicRelayRegistryManager`'s private
    // `validate_and_install` helper to mutate the live registry with it --
    // never construct/mutate this field directly outside this module, so
    // there is exactly one place that decides whether an entry's target is
    // well-formed before it reaches the live snapshot. Deliberately still a
    // raw `String` rather than a validated `RelayTarget`: `store.rs`'s
    // import path must be able to hold an individually-invalid entry just
    // long enough to quarantine it, without failing the whole import.
    target_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

impl PublicRelayEntry {
    pub fn new(
        machine_id: MachineId,
        target_url: impl Into<String>,
        description: Option<String>,
        disabled: bool,
    ) -> Self {
        Self {
            machine_id,
            target_url: target_url.into(),
            description,
            disabled,
        }
    }

    pub fn target_url(&self) -> &str {
        &self.target_url
    }

    pub fn target(&self) -> Result<RelayTarget, PublicRelayError> {
        RelayTarget::parse(self.machine_id.clone(), &self.target_url)
    }
}

#[derive(Debug, Clone)]
pub struct RelayTarget {
    machine_id: MachineId,
    url: Url,
}

impl RelayTarget {
    pub fn parse(machine_id: MachineId, target_url: &str) -> Result<Self, PublicRelayError> {
        let url = Url::parse(target_url)
            .map_err(|error| PublicRelayError::InvalidTarget(error.to_string()))?;
        if url.scheme() != "http" {
            return Err(PublicRelayError::InvalidTarget(
                "scheme must be http".into(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(PublicRelayError::InvalidTarget(
                "userinfo is not allowed".into(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(PublicRelayError::InvalidTarget(
                "query and fragment are not allowed".into(),
            ));
        }
        if url.port_or_known_default() != Some(38935) {
            return Err(PublicRelayError::InvalidTarget("port must be 38935".into()));
        }
        let expected_path = format!("/callback/{}", machine_id.as_str());
        if url.path() != expected_path {
            return Err(PublicRelayError::InvalidTarget(
                "path must match /callback/<machine_id>".into(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| PublicRelayError::InvalidTarget("host is required".into()))?;
        let ip: IpAddr = host
            .parse()
            .map_err(|_| PublicRelayError::InvalidTarget("host must be a Tailscale IP".into()))?;
        if !is_tailscale_cgnat(ip) {
            return Err(PublicRelayError::InvalidTarget(
                "host must be in 100.64.0.0/10".into(),
            ));
        }
        Ok(Self { machine_id, url })
    }

    pub fn machine_id(&self) -> &MachineId {
        &self.machine_id
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn host_str(&self) -> Option<&str> {
        self.url.host_str()
    }

    pub fn port_or_known_default(&self) -> Option<u16> {
        self.url.port_or_known_default()
    }

    pub fn redacted_label(&self) -> String {
        format!(
            "{}@{}",
            self.machine_id,
            self.url.host_str().unwrap_or("unknown")
        )
    }

    #[cfg(test)]
    pub(crate) fn from_validated_parts_for_tests(machine_id: MachineId, url: Url) -> Self {
        Self { machine_id, url }
    }
}

/// Returns true if `ip` falls inside Tailscale's IPv4 CGNAT range
/// (`100.64.0.0/10`, i.e. `100.64.0.0`-`100.127.255.255`).
///
/// IPv6 Tailscale CGNAT addresses (`fd7a:115c:a1e0::/48`) are deliberately
/// out of scope for now and always fail closed (`IpAddr::V6(_) => false`).
/// Do not "fix" this by relaxing the IPv6 arm without first deriving an
/// equivalent IPv6 range check -- doing so naively (e.g. accepting all
/// IPv6) would reopen the SSRF hole this function exists to close.
fn is_tailscale_cgnat(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(_) => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PublicRelaySnapshot {
    pub entries: std::collections::BTreeMap<MachineId, PublicRelayEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub accepted: Vec<String>,
    pub quarantined: Vec<QuarantinedEntry>,
    #[serde(skip_serializing)]
    pub entries: Vec<PublicRelayEntry>,
}

impl ImportReport {
    pub fn empty() -> Self {
        Self {
            accepted: Vec::new(),
            quarantined: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn quarantine_summary(&self) -> Option<String> {
        if self.quarantined.is_empty() {
            return None;
        }
        Some(
            self.quarantined
                .iter()
                .map(|entry| format!("{}: {}", entry.machine_id, entry.reason))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    pub fn ensure_complete_import(&self) -> Result<(), PublicRelayError> {
        if let Some(summary) = self.quarantine_summary() {
            return Err(PublicRelayError::InvalidTarget(format!(
                "registry import contains invalid entries: {summary}"
            )));
        }
        if self.entries.is_empty() {
            return Err(PublicRelayError::InvalidTarget(
                "registry import contains no valid relay machines".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuarantinedEntry {
    pub machine_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryWriteOutcome {
    pub path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub entry_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicRelayMachineView {
    pub machine_id: String,
    pub target: String,
    pub disabled: bool,
    pub description: Option<String>,
}

impl PublicRelayMachineView {
    pub fn from_entry(entry: &PublicRelayEntry) -> Self {
        let target = entry
            .target()
            .map(|target| target.redacted_label())
            .unwrap_or_else(|_| format!("{}@invalid", entry.machine_id));
        Self {
            machine_id: entry.machine_id.to_string(),
            target,
            disabled: entry.disabled,
            description: entry.description.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicRelayHealth {
    pub status: &'static str,
    pub relay: &'static str,
    pub registry: &'static str,
    pub machines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MutationReport {
    pub restart_required: bool,
    pub outcome: RegistryWriteOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_target_accepts_live_tailscale_shape() {
        let machine = MachineId::parse("dookie").unwrap();
        let target =
            RelayTarget::parse(machine, "http://100.88.16.79:38935/callback/dookie").unwrap();
        assert_eq!(
            target.url().as_str(),
            "http://100.88.16.79:38935/callback/dookie"
        );
    }

    #[test]
    fn relay_target_rejects_unsafe_shapes() {
        let cases = [
            "https://100.88.16.79:38935/callback/dookie",
            "http://100.88.16.79:80/callback/dookie",
            "http://127.0.0.1:38935/callback/dookie",
            "http://169.254.169.254:38935/callback/dookie",
            "http://100.88.16.79:38935/callback/other",
            "http://user@100.88.16.79:38935/callback/dookie",
            "http://100.88.16.79:38935/callback/dookie?code=abc",
            "http://100.88.16.79:38935/callback/dookie#frag",
        ];
        for value in cases {
            let machine = MachineId::parse("dookie").unwrap();
            assert!(
                RelayTarget::parse(machine, value).is_err(),
                "{value} should reject"
            );
        }
    }

    #[test]
    fn invalid_param_errors_route_through_tool_error_invalid_param_with_param_field() {
        let cases: Vec<(PublicRelayError, &str)> = vec![
            (
                PublicRelayError::InvalidMachineId("bad id".into()),
                "machine_id",
            ),
            (
                PublicRelayError::InvalidSuffix("bad suffix".into()),
                "suffix",
            ),
            (
                PublicRelayError::InvalidRegistryInput("bad registry".into()),
                "registry",
            ),
            (
                PublicRelayError::InvalidRequestBody("bad body".into()),
                "body",
            ),
        ];

        for (error, expected_param) in cases {
            assert_eq!(error.kind(), "invalid_param");
            let tool_error = error.to_tool_error();
            let value = serde_json::to_value(&tool_error).expect("ToolError should serialize");
            assert_eq!(value["kind"], "invalid_param");
            assert_eq!(
                value["param"], expected_param,
                "expected param field for {error:?}"
            );
            assert!(value.get("message").is_some());
        }
    }

    #[test]
    fn non_invalid_param_errors_still_route_through_sdk_variant() {
        let error = PublicRelayError::ForwarderInitFailed("client build failed".into());
        let tool_error = error.to_tool_error();
        let value = serde_json::to_value(&tool_error).expect("ToolError should serialize");
        assert_eq!(value["kind"], "relay_forwarder_init_failed");
        // The generic Sdk variant has no `param` field.
        assert!(value.get("param").is_none());
    }

    #[test]
    fn is_tailscale_cgnat_boundary_is_100_64_slash_10() {
        // The (64..=127).contains(&octets[1]) check is the CGNAT boundary --
        // exercise both edges on both sides.
        assert!(!is_tailscale_cgnat("100.63.255.255".parse().unwrap()));
        assert!(is_tailscale_cgnat("100.64.0.0".parse().unwrap()));
        assert!(is_tailscale_cgnat("100.127.255.255".parse().unwrap()));
        assert!(!is_tailscale_cgnat("100.128.0.0".parse().unwrap()));
    }

    #[test]
    fn is_tailscale_cgnat_rejects_ipv6_literals() {
        // IPv6 Tailscale CGNAT (fd7a:115c:a1e0::/48) is deliberately out of
        // scope for now -- see the doc comment on `is_tailscale_cgnat`.
        assert!(!is_tailscale_cgnat("fd7a:115c:a1e0::1".parse().unwrap()));
        assert!(!is_tailscale_cgnat("::1".parse().unwrap()));
    }

    #[test]
    fn relay_target_rejects_non_ip_hostname() {
        let machine = MachineId::parse("dookie").unwrap();
        let error = RelayTarget::parse(machine, "http://dookie.example.com:38935/callback/dookie")
            .expect_err("hostname target should reject");
        assert!(matches!(error, PublicRelayError::InvalidTarget(_)));
    }

    #[test]
    fn ensure_complete_import_rejects_empty_but_valid_registry() {
        let report = ImportReport::empty();
        let error = report
            .ensure_complete_import()
            .expect_err("empty import should be rejected");
        assert!(matches!(error, PublicRelayError::InvalidTarget(_)));
        assert!(error.to_string().contains("no valid relay machines"));
    }
}
