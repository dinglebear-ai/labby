//! Non-secret, disabled-by-default native Tailcat preferences.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TailcatPreferences {
    pub enabled: bool,
    pub helper_path: Option<PathBuf>,
    pub helper_sha256: Option<String>,
    pub derp_map_url: Option<String>,
    pub control_socket: Option<PathBuf>,
}

impl TailcatPreferences {
    pub fn socket_path(&self) -> PathBuf {
        self.control_socket
            .clone()
            .unwrap_or_else(|| labby_runtime::lab_home().join("tailcat/control.sock"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_transport_is_disabled_in_empty_configuration() {
        let config: crate::config::LabConfig = toml::from_str("").unwrap();
        assert!(!config.tailcat.enabled);
        assert!(config.tailcat.helper_path.is_none());
        assert!(config.tailcat.helper_sha256.is_none());
    }
    #[test]
    fn unknown_native_transport_options_fail_closed() {
        assert!(
            toml::from_str::<TailcatPreferences>("enabled = true\nallow_host_shell = true")
                .is_err()
        );
    }
}
