use serde::{Deserialize, Serialize};

pub const DEFAULT_PROXY_PORT_RANGE_START: u16 = 49_152;
pub const DEFAULT_PROXY_PORT_RANGE_END: u16 = 65_535;
pub const DEFAULT_PROXY_SHUTDOWN_GRACE_MS: u64 = 3_000;
pub const DEFAULT_PROXY_BEARER_TOKEN_ENV: &str = "LABBY_PROXY_BEARER_TOKEN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyExposure {
    #[default]
    Tailscale,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAuthMode {
    #[default]
    Tailnet,
    Bearer,
    Oauth,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPortMode {
    #[default]
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProxyPortPreference {
    Fixed(u16),
    Mode(ProxyPortMode),
}

impl Default for ProxyPortPreference {
    fn default() -> Self {
        Self::Mode(ProxyPortMode::Random)
    }
}

impl ProxyPortPreference {
    #[must_use]
    pub const fn fixed(self) -> Option<u16> {
        match self {
            Self::Fixed(port) => Some(port),
            Self::Mode(ProxyPortMode::Random) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyPreferences {
    #[serde(default)]
    pub exposure: ProxyExposure,
    #[serde(default)]
    pub auth: ProxyAuthMode,
    #[serde(default = "default_proxy_path")]
    pub path: String,
    #[serde(default)]
    pub port: ProxyPortPreference,
    #[serde(default = "default_proxy_port_range_start")]
    pub port_range_start: u16,
    #[serde(default = "default_proxy_port_range_end")]
    pub port_range_end: u16,
    #[serde(default = "default_proxy_bearer_token_env")]
    pub bearer_token_env: String,
    #[serde(default = "default_proxy_oauth_scopes")]
    pub oauth_scopes: Vec<String>,
    #[serde(default)]
    pub inherit_env: Vec<String>,
    #[serde(default = "default_proxy_shutdown_grace_ms")]
    pub shutdown_grace_ms: u64,
}

impl Default for ProxyPreferences {
    fn default() -> Self {
        Self {
            exposure: ProxyExposure::Tailscale,
            auth: ProxyAuthMode::Tailnet,
            path: default_proxy_path(),
            port: ProxyPortPreference::default(),
            port_range_start: DEFAULT_PROXY_PORT_RANGE_START,
            port_range_end: DEFAULT_PROXY_PORT_RANGE_END,
            bearer_token_env: default_proxy_bearer_token_env(),
            oauth_scopes: default_proxy_oauth_scopes(),
            inherit_env: Vec::new(),
            shutdown_grace_ms: DEFAULT_PROXY_SHUTDOWN_GRACE_MS,
        }
    }
}

impl ProxyPreferences {
    pub fn validate(&self) -> Result<(), ProxyConfigError> {
        validate_proxy_path(&self.path)?;
        if self.port_range_start > self.port_range_end {
            return Err(ProxyConfigError::InvalidPortRange {
                start: self.port_range_start,
                end: self.port_range_end,
            });
        }
        if self.port_range_start < 1_024 {
            return Err(ProxyConfigError::PrivilegedPortRange {
                start: self.port_range_start,
            });
        }
        if matches!(self.port, ProxyPortPreference::Fixed(0)) {
            return Err(ProxyConfigError::InvalidFixedPort);
        }
        if matches!(self.exposure, ProxyExposure::Local)
            && matches!(self.auth, ProxyAuthMode::Tailnet)
        {
            return Err(ProxyConfigError::TailnetAuthRequiresTailscale);
        }
        if self.bearer_token_env.trim().is_empty() {
            return Err(ProxyConfigError::EmptyBearerTokenEnv);
        }
        if !is_env_name(&self.bearer_token_env) {
            return Err(ProxyConfigError::InvalidEnvName {
                name: self.bearer_token_env.clone(),
            });
        }
        for name in &self.inherit_env {
            if !is_env_name(name) {
                return Err(ProxyConfigError::InvalidEnvName { name: name.clone() });
            }
        }
        if matches!(self.auth, ProxyAuthMode::Oauth) && self.oauth_scopes.is_empty() {
            return Err(ProxyConfigError::MissingOauthScopes);
        }
        if self
            .oauth_scopes
            .iter()
            .any(|scope| scope.trim().is_empty() || scope.chars().any(char::is_whitespace))
        {
            return Err(ProxyConfigError::InvalidOauthScope);
        }
        if !(1..=60_000).contains(&self.shutdown_grace_ms) {
            return Err(ProxyConfigError::InvalidShutdownGrace {
                value: self.shutdown_grace_ms,
            });
        }
        Ok(())
    }
}

fn validate_proxy_path(path: &str) -> Result<(), ProxyConfigError> {
    let path = path.trim();
    if path.is_empty()
        || path == "/"
        || !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ProxyConfigError::InvalidPath {
            path: path.to_string(),
        });
    }
    Ok(())
}

fn is_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn default_proxy_path() -> String {
    "/mcp".to_string()
}

const fn default_proxy_port_range_start() -> u16 {
    DEFAULT_PROXY_PORT_RANGE_START
}

const fn default_proxy_port_range_end() -> u16 {
    DEFAULT_PROXY_PORT_RANGE_END
}

fn default_proxy_bearer_token_env() -> String {
    DEFAULT_PROXY_BEARER_TOKEN_ENV.to_string()
}

fn default_proxy_oauth_scopes() -> Vec<String> {
    vec!["mcp:read".to_string(), "mcp:write".to_string()]
}

const fn default_proxy_shutdown_grace_ms() -> u64 {
    DEFAULT_PROXY_SHUTDOWN_GRACE_MS
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProxyConfigError {
    #[error(
        "proxy path `{path}` must be an absolute non-root path without query, fragment, or dot segments"
    )]
    InvalidPath { path: String },
    #[error("proxy port range start {start} exceeds end {end}")]
    InvalidPortRange { start: u16, end: u16 },
    #[error("proxy random port range must start at 1024 or higher, got {start}")]
    PrivilegedPortRange { start: u16 },
    #[error("proxy fixed port must not be zero")]
    InvalidFixedPort,
    #[error("proxy auth `tailnet` requires Tailscale exposure")]
    TailnetAuthRequiresTailscale,
    #[error("proxy bearer token environment key must not be empty")]
    EmptyBearerTokenEnv,
    #[error("invalid environment variable name `{name}`")]
    InvalidEnvName { name: String },
    #[error("proxy OAuth mode requires at least one scope")]
    MissingOauthScopes,
    #[error("proxy OAuth scopes must be non-empty single tokens")]
    InvalidOauthScope,
    #[error("proxy shutdown_grace_ms={value} is invalid; expected 1..=60000")]
    InvalidShutdownGrace { value: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preferences_are_zero_flag_safe() {
        let cfg = ProxyPreferences::default();
        assert_eq!(cfg.exposure, ProxyExposure::Tailscale);
        assert_eq!(cfg.auth, ProxyAuthMode::Tailnet);
        assert_eq!(cfg.path, "/mcp");
        assert_eq!(cfg.port.fixed(), None);
        assert_eq!(cfg.port_range_start, 49_152);
        assert_eq!(cfg.port_range_end, 65_535);
        assert_eq!(cfg.bearer_token_env, "LABBY_PROXY_BEARER_TOKEN");
        cfg.validate().expect("default proxy preferences validate");
    }

    #[test]
    fn toml_accepts_random_and_fixed_ports() {
        let random: ProxyPreferences = toml::from_str(r#"port = "random""#).unwrap();
        assert_eq!(random.port.fixed(), None);
        let fixed: ProxyPreferences = toml::from_str("port = 52177").unwrap();
        assert_eq!(fixed.port.fixed(), Some(52_177));
    }

    #[test]
    fn local_tailnet_combination_is_rejected() {
        let cfg = ProxyPreferences {
            exposure: ProxyExposure::Local,
            ..ProxyPreferences::default()
        };
        assert_eq!(
            cfg.validate(),
            Err(ProxyConfigError::TailnetAuthRequiresTailscale)
        );
    }

    #[test]
    fn invalid_paths_are_rejected() {
        for path in ["", "/", "mcp", "/mcp?x=1", "/mcp#x", "/a/../b"] {
            let cfg = ProxyPreferences {
                path: path.to_string(),
                ..ProxyPreferences::default()
            };
            assert!(matches!(
                cfg.validate(),
                Err(ProxyConfigError::InvalidPath { .. })
            ));
        }
    }

    #[test]
    fn invalid_environment_names_are_rejected() {
        let cfg = ProxyPreferences {
            inherit_env: vec!["GOOD_NAME".into(), "bad-name".into()],
            ..ProxyPreferences::default()
        };
        assert_eq!(
            cfg.validate(),
            Err(ProxyConfigError::InvalidEnvName {
                name: "bad-name".into()
            })
        );
    }
}
