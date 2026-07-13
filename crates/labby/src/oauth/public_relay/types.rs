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
        if !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        if trimmed == "." || trimmed == ".." {
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
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("registry unavailable: {0}")]
    RegistryUnavailable(String),
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
            Self::InvalidMachineId(_) | Self::InvalidSuffix(_) => "invalid_param",
            Self::InvalidTarget(_) => "relay_invalid_target",
            Self::RegistryUnavailable(_) => "relay_registry_unavailable",
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
        ToolError::Sdk {
            sdk_kind: self.kind().to_string(),
            message: self.to_string(),
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::Overloaded => "relay busy; retry later",
            Self::BodyTooLarge => "callback request too large",
            Self::ResponseTooLarge => "callback response too large",
            Self::UpstreamTimeout => "callback target timed out",
            Self::UpstreamError => "callback target unavailable",
            _ => "callback target unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicRelayEntry {
    pub machine_id: MachineId,
    pub target_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

impl PublicRelayEntry {
    pub fn target(&self) -> Result<RelayTarget, PublicRelayError> {
        RelayTarget::parse(self.machine_id.clone(), &self.target_url)
    }
}

#[derive(Debug, Clone)]
pub struct RelayTarget {
    pub machine_id: MachineId,
    pub url: Url,
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

    pub fn redacted_label(&self) -> String {
        format!(
            "{}@{}",
            self.machine_id,
            self.url.host_str().unwrap_or("unknown")
        )
    }
}

fn is_tailscale_cgnat(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(_) => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
            target.url.as_str(),
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
}
