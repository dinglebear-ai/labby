//! Product-local discovery configuration. Resolution performs no I/O.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const PUBLIC_ID: &str = "public";
pub const LEGACY_ID: &str = "legacy";
pub const PUBLIC_ENDPOINT: &str = "https://depot.dinglebear.ai";
/// Default environment variable holding the managed-authority bearer token
/// Labby presents to Depot's authority inbox (secret; never persisted).
pub const DEFAULT_AUTHORITY_BEARER_TOKEN_ENV: &str = "LABBY_DEPOT_AUTHORITY_TOKEN";
/// Default environment variable holding the base64url (no padding) 32-byte
/// Ed25519 seed used to sign authority projections and delegated assertions
/// (secret; never persisted).
pub const DEFAULT_AUTHORITY_SIGNING_KEY_ENV: &str = "LABBY_DEPOT_AUTHORITY_SIGNING_KEY";
/// Bounded overlapping key rotation: at most this many additional signing
/// keys may be registered beside the active one.
pub const MAX_AUTHORITY_OVERLAP_KEYS: usize = 7;
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_TOMBSTONES: usize = 4096;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepotControlMode {
    #[default]
    Standalone,
    LabbyManaged,
}

/// Wire generations remain opaque strings even if an upstream uses numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpaqueEpoch(String);

impl TryFrom<String> for OpaqueEpoch {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !(1..=128).contains(&value.chars().count()) {
            return Err("invalid_epoch");
        }
        Ok(Self(value))
    }
}

impl From<OpaqueEpoch> for String {
    fn from(value: OpaqueEpoch) -> Self {
        value.0
    }
}

/// Raw entries retain malformed siblings and future fields for targeted edits.
/// Never use this disk model as an HTTP response; use `ResolvedDepot` instead.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DepotPreferences {
    pub control_mode: DepotControlMode,
    pub managed_authority_kill_switch: bool,
    /// Single host-selected project whose members may read these catalogs.
    pub read_project_id: Option<String>,
    /// Host-file-only loopback services. Never writable through provider APIs.
    pub local_providers: Vec<LocalProviderConfig>,
    /// Host-file-only authenticated catalog binding for the reserved public ID.
    pub public_read_binding: Option<PublicReadBinding>,
    /// Explicit server-owned browser publishing target; never selected by a request.
    pub publish: Option<DepotPublishTarget>,
    pub public_enabled: bool,
    pub providers: Vec<toml::Value>,
    pub tombstones: BTreeSet<String>,
    pub legacy_migrated: bool,
    /// Managed authority replication target. Secret material is referenced by
    /// environment-variable *name* only and resolved whenever a managed
    /// control plane or projection sender is constructed (daemon start and
    /// every later rebuild); values are never serialized into projections.
    pub authority_endpoint: Option<String>,
    /// Environment variable holding the Depot authority bearer token. Absent
    /// means [`DEFAULT_AUTHORITY_BEARER_TOKEN_ENV`].
    pub authority_bearer_token_env: Option<String>,
    pub authority_installation_id: Option<String>,
    /// Key ID stamped into projection envelopes and delegated assertions.
    pub authority_key_id: Option<String>,
    /// Environment variable holding the active Ed25519 signing seed as
    /// base64url without padding (32 bytes decoded). Absent means
    /// [`DEFAULT_AUTHORITY_SIGNING_KEY_ENV`].
    pub authority_signing_key_env: Option<String>,
    /// Additional signing keys kept valid during rotation. Each entry names a
    /// key ID and the environment variable holding its seed; the active key
    /// stays `authority_key_id`/`authority_signing_key_env`.
    pub authority_overlap_signing_keys: Vec<AuthorityOverlapSigningKey>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// One retired-but-still-valid signing key registered for rotation overlap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityOverlapSigningKey {
    pub key_id: String,
    pub signing_key_env: String,
}

impl Default for DepotPreferences {
    fn default() -> Self {
        Self {
            control_mode: DepotControlMode::Standalone,
            managed_authority_kill_switch: false,
            read_project_id: None,
            local_providers: Vec::new(),
            public_read_binding: None,
            publish: None,
            public_enabled: true,
            providers: Vec::new(),
            tombstones: BTreeSet::new(),
            legacy_migrated: false,
            authority_endpoint: None,
            authority_bearer_token_env: None,
            authority_installation_id: None,
            authority_key_id: None,
            authority_signing_key_env: None,
            authority_overlap_signing_keys: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepotPublishTarget {
    pub route_id: String,
    pub project_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalProviderConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub bearer_token_env: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicReadBinding {
    pub endpoint: String,
    pub bearer_token_env: String,
    pub deployment_id: OpaqueEpoch,
}

impl std::fmt::Debug for DepotPreferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepotPreferences")
            .field("public_enabled", &self.public_enabled)
            .field("control_mode", &self.control_mode)
            .field(
                "managed_authority_kill_switch",
                &self.managed_authority_kill_switch,
            )
            .field("provider_count", &self.providers.len())
            .field("tombstone_count", &self.tombstones.len())
            .field("legacy_migrated", &self.legacy_migrated)
            .field(
                "authority_endpoint_configured",
                &self.authority_endpoint.is_some(),
            )
            .field(
                "authority_bearer_token_env",
                &self.authority_bearer_token_env(),
            )
            .field(
                "authority_signing_key_env",
                &self.authority_signing_key_env(),
            )
            .field(
                "authority_overlap_key_count",
                &self.authority_overlap_signing_keys.len(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Anonymous,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub auth_mode: AuthMode,
    pub bearer_token_env: Option<String>,
}

/// Safe configuration projection. Credentials and raw diagnostics never cross
/// the surface boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub auth_mode: AuthMode,
    #[serde(skip)]
    pub bearer_token_env: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub host_managed: bool,
    #[serde(skip)]
    pub read_project_id: Option<String>,
    #[serde(skip)]
    pub expected_deployment_id: Option<OpaqueEpoch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDiagnostic {
    pub entry_index: usize,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedDepot {
    pub providers: Vec<ProviderView>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

/// Only presence is needed during normalization. Secret values are resolved
/// later from an immutable server-held snapshot.
#[derive(Debug, Clone, Default)]
pub struct LegacyDepot {
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub token_present: bool,
}

impl DepotPreferences {
    /// Environment variable name for the authority bearer token, applying the
    /// documented default.
    #[must_use]
    pub fn authority_bearer_token_env(&self) -> &str {
        self.authority_bearer_token_env
            .as_deref()
            .unwrap_or(DEFAULT_AUTHORITY_BEARER_TOKEN_ENV)
    }

    /// Environment variable name for the active signing seed, applying the
    /// documented default.
    #[must_use]
    pub fn authority_signing_key_env(&self) -> &str {
        self.authority_signing_key_env
            .as_deref()
            .unwrap_or(DEFAULT_AUTHORITY_SIGNING_KEY_ENV)
    }

    /// `(key_id, env)` pairs for every registered signing key, active first.
    /// Fails closed on an invalid reference, a duplicate key ID, or more than
    /// the bounded overlap set.
    pub fn authority_signing_key_envs(&self) -> Result<Vec<(String, String)>, &'static str> {
        let active_id = self
            .authority_key_id
            .as_deref()
            .ok_or("authority_key_id_required")?;
        let active_env = self.authority_signing_key_env();
        if !allowed_secret_reference(active_env) {
            return Err("invalid_signing_key_reference");
        }
        if self.authority_overlap_signing_keys.len() > MAX_AUTHORITY_OVERLAP_KEYS {
            return Err("too_many_overlap_keys");
        }
        let mut keys = vec![(active_id.to_owned(), active_env.to_owned())];
        for overlap in &self.authority_overlap_signing_keys {
            if overlap.key_id.trim().is_empty()
                || !allowed_secret_reference(&overlap.signing_key_env)
                || keys.iter().any(|(id, _)| id == &overlap.key_id)
            {
                return Err("invalid_overlap_key");
            }
            keys.push((overlap.key_id.clone(), overlap.signing_key_env.clone()));
        }
        Ok(keys)
    }

    /// Managed mode must never fall back to standalone authority when its
    /// projection path is stale, disabled, or on an unknown protocol version.
    #[must_use]
    pub fn managed_mutations_ready(&self, projection_ready: bool, protocol_version: u64) -> bool {
        self.control_mode == DepotControlMode::LabbyManaged
            && !self.managed_authority_kill_switch
            && projection_ready
            && protocol_version == 1
    }

    #[must_use]
    pub fn resolve(&self, legacy: &LegacyDepot) -> ResolvedDepot {
        let mut result = ResolvedDepot {
            providers: vec![ProviderView {
                id: PUBLIC_ID.into(),
                name: "Public Depot".into(),
                endpoint: PUBLIC_ENDPOINT.into(),
                enabled: self.public_enabled,
                auth_mode: AuthMode::Anonymous,
                bearer_token_env: None,
                host_managed: false,
                read_project_id: None,
                expected_deployment_id: None,
            }],
            diagnostics: Vec::new(),
        };
        // A malformed host binding must never fall back to an unscoped catalog.
        if self.validate_local_providers().is_err() {
            result.providers.clear();
            result.diagnostics.push(ConfigDiagnostic {
                entry_index: 0,
                kind: "invalid_local_provider_binding",
            });
            return result;
        }
        if let Some(binding) = &self.public_read_binding {
            let public = &mut result.providers[0];
            public.endpoint = binding.endpoint.clone();
            public.auth_mode = AuthMode::Bearer;
            public.bearer_token_env = Some(binding.bearer_token_env.clone());
            public.host_managed = true;
            public.read_project_id = self.read_project_id.clone();
            public.expected_deployment_id = Some(binding.deployment_id.clone());
        }
        result
            .providers
            .extend(self.local_providers.iter().map(|p| ProviderView {
                id: p.id.clone(),
                name: p.name.clone(),
                endpoint: p.endpoint.clone(),
                enabled: true,
                auth_mode: AuthMode::Bearer,
                bearer_token_env: Some(p.bearer_token_env.clone()),
                host_managed: true,
                read_project_id: self.read_project_id.clone(),
                expected_deployment_id: None,
            }));
        if self.tombstones.len() > MAX_TOMBSTONES {
            result.diagnostics.push(ConfigDiagnostic {
                entry_index: 0,
                kind: "tombstone_capacity",
            });
            return result;
        }
        // Count IDs before selecting any entry, including malformed siblings.
        let mut counts = BTreeMap::<&str, usize>::new();
        for raw in &self.providers {
            if let Some(id) = raw.get("id").and_then(toml::Value::as_str) {
                *counts.entry(id).or_default() += 1;
            }
        }
        let pending_legacy = !self.legacy_migrated
            && !self.tombstones.contains(LEGACY_ID)
            && (legacy.url.is_some() || legacy.enabled.is_some() || legacy.token_present);
        let slots = MAX_PROVIDERS
            .saturating_sub(1 + self.local_providers.len() + usize::from(pending_legacy));
        for (index, raw) in self.providers.iter().take(MAX_PROVIDERS).enumerate() {
            let parsed = raw.clone().try_into::<ProviderConfig>();
            let checked = parsed.map_err(|_| "invalid_entry").and_then(|provider| {
                if index >= slots {
                    return Err("provider_capacity");
                }
                if counts.get(provider.id.as_str()).copied().unwrap_or(0) != 1 {
                    return Err("duplicate_id");
                }
                if provider.id == PUBLIC_ID
                    || provider.id == "all"
                    || (provider.id == LEGACY_ID && !self.legacy_migrated)
                {
                    return Err("reserved_id");
                }
                if self.tombstones.contains(&provider.id) {
                    return Err("removed_id");
                }
                provider.validate()?;
                Ok(provider)
            });
            match checked {
                Ok(provider) => result.providers.push(provider.into()),
                Err(kind) => result.diagnostics.push(ConfigDiagnostic {
                    entry_index: index,
                    kind,
                }),
            }
        }
        if self.providers.len() > MAX_PROVIDERS {
            result.diagnostics.push(ConfigDiagnostic {
                entry_index: MAX_PROVIDERS,
                kind: "provider_capacity",
            });
        }
        if pending_legacy {
            let provider = ProviderConfig {
                id: LEGACY_ID.into(),
                name: "Legacy Depot".into(),
                endpoint: legacy.url.clone().unwrap_or_default(),
                enabled: legacy.enabled.unwrap_or(true),
                auth_mode: AuthMode::Bearer,
                bearer_token_env: Some("LABBY_DEPOT_TOKEN".into()),
            };
            let error = if counts.contains_key(LEGACY_ID) {
                Some("legacy_collision")
            } else if legacy.url.is_none() {
                Some("legacy_url_required")
            } else if provider.enabled && !legacy.token_present {
                Some("credential_required")
            } else {
                provider.validate().err()
            };
            if let Some(kind) = error {
                result.diagnostics.push(ConfigDiagnostic {
                    entry_index: MAX_PROVIDERS,
                    kind,
                });
            } else {
                result.providers.push(provider.into());
            }
        }
        result
    }

    pub fn validate_local_providers(&self) -> Result<(), &'static str> {
        if self.local_providers.is_empty() && self.public_read_binding.is_none() {
            return Ok(());
        }
        if self
            .read_project_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty() || id.trim() != id)
            || self.local_providers.len() > MAX_PROVIDERS - 2
        {
            return Err("invalid local Depot project binding");
        }
        let mut ids = BTreeSet::new();
        let mut references = BTreeSet::new();
        if let Some(binding) = &self.public_read_binding {
            if canonical_local_endpoint(&binding.endpoint).is_err()
                || !allowed_secret_reference(&binding.bearer_token_env)
                || binding.bearer_token_env == "LABBY_DEPOT_TOKEN"
                || self.providers.iter().any(|raw| {
                    raw.get("bearer_token_env").and_then(toml::Value::as_str)
                        == Some(&binding.bearer_token_env)
                })
            {
                return Err("invalid Public Depot read binding");
            }
            references.insert(&binding.bearer_token_env);
        }
        for local in &self.local_providers {
            if !valid_provider_id(&local.id)
                || matches!(local.id.as_str(), PUBLIC_ID | LEGACY_ID)
                || !ids.insert(&local.id)
                || !(1..=128).contains(&local.name.chars().count())
                || canonical_local_endpoint(&local.endpoint).is_err()
                || !allowed_secret_reference(&local.bearer_token_env)
                || local.bearer_token_env == "LABBY_DEPOT_TOKEN"
                || !references.insert(&local.bearer_token_env)
                || self.tombstones.contains(&local.id)
                || self.providers.iter().any(|raw| {
                    raw.get("id").and_then(toml::Value::as_str) == Some(&local.id)
                        || raw.get("bearer_token_env").and_then(toml::Value::as_str)
                            == Some(&local.bearer_token_env)
                })
            {
                return Err("invalid local Depot provider binding");
            }
        }
        Ok(())
    }
    pub fn host_read_bindings(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.local_providers
            .iter()
            .map(|p| {
                (
                    p.id.as_str(),
                    p.endpoint.as_str(),
                    p.bearer_token_env.as_str(),
                )
            })
            .chain(
                self.public_read_binding
                    .iter()
                    .map(|p| (PUBLIC_ID, p.endpoint.as_str(), p.bearer_token_env.as_str())),
            )
    }

    pub fn validate_public_acquisition(
        &self,
        artifacts: &super::ArtifactPreferences,
    ) -> Result<(), &'static str> {
        self.validate_local_providers()?;
        let Some(binding) = &self.public_read_binding else {
            return Ok(());
        };
        if artifacts.sources.iter().any(|source| {
            source.id != PUBLIC_ID
                && source.bearer_token_env.as_deref() == Some(&binding.bearer_token_env)
        }) {
            return Err("Public Depot credential cannot be reused by another acquisition source");
        }
        let mut sources = artifacts
            .sources
            .iter()
            .filter(|source| source.id == PUBLIC_ID);
        let source = sources
            .next()
            .ok_or("Public Depot exact acquisition connection required")?;
        if sources.next().is_some()
            || source.kind != super::ArtifactSourceKind::Depot
            || source.bearer_token_env.as_deref() != Some(&binding.bearer_token_env)
        {
            return Err("Public Depot acquisition credential binding mismatch");
        }
        let endpoint = canonical_endpoint(&source.endpoint)
            .map_err(|_| "invalid Public Depot acquisition endpoint")?;
        if !url::Url::parse(&source.endpoint).is_ok_and(|url| url.path() == "/api/artifacts/exact")
        {
            return Err("Public Depot exact acquisition endpoint required");
        }
        if let Some(control) = &source.control_plane_url {
            let control =
                canonical_endpoint(control).map_err(|_| "invalid Public Depot control origin")?;
            if control.origin() != endpoint.origin() || control.path() != "/" {
                return Err("Public Depot control origin mismatch");
            }
        }
        Ok(())
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_provider_id(&self.id) {
            return Err("invalid_id");
        }
        if !(1..=128).contains(&self.name.chars().count()) {
            return Err("invalid_name");
        }
        canonical_endpoint(&self.endpoint)?;
        match (&self.auth_mode, &self.bearer_token_env) {
            (AuthMode::Anonymous, Some(_)) => Err("unexpected_credential"),
            (AuthMode::Bearer, Some(key)) if allowed_secret_reference(key) => Ok(()),
            (AuthMode::Bearer, _) => Err("credential_reference_required"),
            _ => Ok(()),
        }
    }
}

impl From<ProviderConfig> for ProviderView {
    fn from(p: ProviderConfig) -> Self {
        Self {
            id: p.id,
            name: p.name,
            endpoint: p.endpoint,
            enabled: p.enabled,
            auth_mode: p.auth_mode,
            bearer_token_env: p.bearer_token_env,
            host_managed: false,
            read_project_id: None,
            expected_deployment_id: None,
        }
    }
}

pub fn valid_provider_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id != "all"
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Secret references are environment-variable names in the `LABBY_DEPOT_*`
/// namespace ending in `_TOKEN` (bearer values) or `_KEY` (signing seeds).
pub fn allowed_secret_reference(key: &str) -> bool {
    key.len() <= 128
        && key.starts_with("LABBY_DEPOT_")
        && (key.ends_with("_TOKEN") || key.ends_with("_KEY"))
        && key
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
}

pub fn canonical_endpoint(raw: &str) -> Result<url::Url, &'static str> {
    if raw.len() > 2048 || raw.trim() != raw {
        return Err("invalid_endpoint");
    }
    let mut url = url::Url::parse(raw).map_err(|_| "invalid_endpoint")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_endpoint");
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

/// Deliberately inspect the raw authority before URL normalization: alternate
/// IPv4 encodings and IPv6 spellings must not acquire a loopback capability.
pub fn canonical_local_endpoint(raw: &str) -> Result<url::Url, &'static str> {
    let authority = raw
        .strip_prefix("http://")
        .ok_or("invalid_local_endpoint")?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    let port = authority
        .strip_prefix("127.0.0.1:")
        .or_else(|| authority.strip_prefix("[::1]:"))
        .ok_or("invalid_local_endpoint")?;
    if port.is_empty()
        || !port.bytes().all(|c| c.is_ascii_digit())
        || port.starts_with('0')
        || port.parse::<u16>().ok().is_none_or(|port| port == 0)
    {
        return Err("invalid_local_endpoint");
    }
    url::Url::parse(raw).map_err(|_| "invalid_local_endpoint")
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ArtifactRefWire")]
pub struct ArtifactRef {
    pub provider_id: String,
    pub artifact_id: String,
}

#[derive(Deserialize)]
struct ArtifactRefWire {
    provider_id: String,
    artifact_id: String,
}

impl TryFrom<ArtifactRefWire> for ArtifactRef {
    type Error = &'static str;
    fn try_from(raw: ArtifactRefWire) -> Result<Self, Self::Error> {
        Self::new(&raw.provider_id, &raw.artifact_id)
    }
}

impl ArtifactRef {
    pub fn new(provider_id: &str, artifact_id: &str) -> Result<Self, &'static str> {
        if !valid_provider_id(provider_id) || !(1..=2048).contains(&artifact_id.len()) {
            return Err("invalid_artifact_identity");
        }
        Ok(Self {
            provider_id: provider_id.into(),
            artifact_id: artifact_id.into(),
        })
    }
}

pub fn safe_total(value: u64) -> Option<u64> {
    (value <= MAX_SAFE_INTEGER).then_some(value)
}

#[cfg(test)]
mod publish_target_tests {
    use super::*;
    #[test]
    fn publishing_has_no_default_target_and_round_trips_explicit_server_binding() {
        assert!(DepotPreferences::default().publish.is_none());
        let config: DepotPreferences =
            toml::from_str("[publish]\nroute_id='team-publish'\nproject_id='team-project'\n")
                .unwrap();
        assert_eq!(config.publish.as_ref().unwrap().route_id, "team-publish");
        let restored: DepotPreferences =
            toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(config.publish, restored.publish);
        assert!(
            toml::from_str::<DepotPreferences>(
                "[publish]\nroute_id='team'\nproject_id='project'\nprincipal_id='owner'\n"
            )
            .is_err()
        );
    }

    #[test]
    fn local_endpoint_accepts_only_exact_loopback_literals_and_explicit_ports() {
        for valid in [
            "http://127.0.0.1:4100",
            "http://127.0.0.1:80/",
            "http://[::1]:4101/",
        ] {
            assert!(canonical_local_endpoint(valid).is_ok(), "{valid}");
            assert!(canonical_endpoint(valid).is_err());
        }
        for invalid in [
            "http://localhost:4100",
            "https://127.0.0.1:4100",
            "http://127.0.0.1",
            "http://127.0.0.2:4100",
            "http://127.1:4100",
            "http://2130706433:4100",
            "http://0x7f000001:4100",
            "http://0177.0.0.1:4100",
            "http://127.000.000.001:4100",
            "http://[0:0:0:0:0:0:0:1]:4100",
            "http://[::ffff:127.0.0.1]:4100",
            "http://[::1%25lo]:4100",
            "http://127.0.0.1:0",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:04100",
            "http://127.0.0.1:4100//",
            "http://127.0.0.1:4100/api",
            "http://127.0.0.1:4100/?x",
            "http://127.0.0.1:4100/#x",
            "http://x@127.0.0.1:4100",
            " http://127.0.0.1:4100",
            "http://127.0.0.1:4100\n",
            "http://127.0.0.1:4100/../",
            "http://127.0.0.1:4100\\",
            "http://10.0.0.1:4100",
            "http://169.254.169.254:80",
        ] {
            assert!(canonical_local_endpoint(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn local_config_requires_scope_and_reserves_ids_and_credentials() {
        let mut config: DepotPreferences = toml::from_str(
            r#"
read_project_id = "team-project"
[[local_providers]]
id = "team-local"
name = "Team"
endpoint = "http://127.0.0.1:4100"
bearer_token_env = "LABBY_DEPOT_TEAM_READ_TOKEN"
"#,
        )
        .unwrap();
        assert!(config.validate_local_providers().is_ok());
        let resolved = config.resolve(&LegacyDepot::default());
        assert!(resolved.providers[1].host_managed);
        assert_eq!(
            resolved.providers[1].read_project_id.as_deref(),
            Some("team-project")
        );
        let public = serde_json::to_value(&resolved.providers[1]).unwrap();
        assert!(public.get("bearerTokenEnv").is_none());
        assert!(public.get("readProjectId").is_none());
        config.read_project_id = None;
        assert!(config.validate_local_providers().is_err());
        assert!(config.resolve(&LegacyDepot::default()).providers.is_empty());
        config.read_project_id = Some("team-project".into());
        config
            .providers
            .push(toml::toml! { id = "team-local" }.into());
        assert!(config.validate_local_providers().is_err());
        config.providers.clear();
        config
            .local_providers
            .push(config.local_providers[0].clone());
        assert!(config.validate_local_providers().is_err());
    }

    #[test]
    fn local_capacity_reserves_public_and_pending_legacy_slots() {
        let mut config = DepotPreferences {
            read_project_id: Some("team".into()),
            ..Default::default()
        };
        for index in 0..14 {
            config.local_providers.push(LocalProviderConfig {
                id: format!("local-{index}"),
                name: format!("Local {index}"),
                endpoint: format!("http://127.0.0.1:{}", 4100 + index),
                bearer_token_env: format!("LABBY_DEPOT_LOCAL_{index}_TOKEN"),
            });
        }
        let legacy = LegacyDepot {
            url: Some("https://legacy.example".into()),
            enabled: Some(true),
            token_present: true,
        };
        assert_eq!(config.resolve(&legacy).providers.len(), MAX_PROVIDERS);
        config.local_providers.push(LocalProviderConfig {
            id: "overflow".into(),
            name: "Overflow".into(),
            endpoint: "http://127.0.0.1:4200".into(),
            bearer_token_env: "LABBY_DEPOT_OVERFLOW_TOKEN".into(),
        });
        assert!(config.validate_local_providers().is_err());
        assert!(config.resolve(&legacy).providers.is_empty());
    }
}
