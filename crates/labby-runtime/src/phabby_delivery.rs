//! Surface-neutral projection of the proposed Depot-to-Labby delivery v1 wire contract.
//!
//! This module validates untrusted contract data. It deliberately does not authorize
//! principals, access databases, fetch bytes, or implement transfer state persistence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    net::IpAddr,
};

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::artifacts::canonical_json;

pub const DELIVERY_SCHEMA: &str = "dinglebear.depot-delivery/v1";
pub const MANIFEST_SCHEMA: &str = "dinglebear.depot-delivery-manifest/v1";
pub const ARTIFACT_SCHEMA: &str = "dinglebear.artifact-interchange/v1";

const MAX_COMPONENTS: usize = 2_000;
const MAX_EDGES: usize = 8_000;
const MAX_DEPTH: usize = 32;
const MAX_CHUNKS: usize = 4_096;
const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;
const MAX_COMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_UNCOMPRESSED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_WIRE_BYTES: usize = 16 * 1024 * 1024;
const MAX_WIRE_COLLECTION_ITEMS: usize = 8_192;
const MAX_WIRE_DEPTH: usize = 64;
const MAX_WIRE_STRING_BYTES: usize = 16_384;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("invalid {field}: {reason}")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("input is not canonical JSON")]
    NonCanonical,
    #[error("contract identities do not match at {0}")]
    IdentityMismatch(&'static str),
}

fn invalid(field: &'static str, reason: &'static str) -> ContractError {
    ContractError::Invalid { field, reason }
}

pub trait Validate {
    fn validate(&self) -> Result<(), ContractError>;
}

struct PreflightSeed(usize);

impl<'de> DeserializeSeed<'de> for PreflightSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        if self.0 > MAX_WIRE_DEPTH {
            return Err(de::Error::custom("wire nesting limit exceeded"));
        }
        deserializer.deserialize_any(PreflightVisitor(self.0))
    }
}

struct PreflightVisitor(usize);

impl<'de> Visitor<'de> for PreflightVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON")
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        PreflightSeed(self.0 + 1).deserialize(deserializer)
    }

    fn visit_str<E>(self, value: &str) -> Result<(), E>
    where
        E: de::Error,
    {
        if value.len() > MAX_WIRE_STRING_BYTES {
            return Err(E::custom("wire string limit exceeded"));
        }
        Ok(())
    }

    fn visit_string<E>(self, value: String) -> Result<(), E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut count = 0;
        while sequence
            .next_element_seed(PreflightSeed(self.0 + 1))?
            .is_some()
        {
            count += 1;
            if count > MAX_WIRE_COLLECTION_ITEMS {
                return Err(de::Error::custom("wire collection limit exceeded"));
            }
        }
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut count = 0;
        while let Some(key) = map.next_key::<String>()? {
            count += 1;
            if count > MAX_WIRE_COLLECTION_ITEMS {
                return Err(de::Error::custom("wire collection limit exceeded"));
            }
            if key.len() > 256 {
                return Err(de::Error::custom("wire key limit exceeded"));
            }
            map.next_value_seed(PreflightSeed(self.0 + 1))?;
        }
        Ok(())
    }
}

fn preflight(bytes: &[u8]) -> Result<(), ContractError> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(invalid("wire", "byte_limit_exceeded"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    PreflightSeed(0)
        .deserialize(&mut deserializer)
        .map_err(|error| ContractError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| ContractError::Json(error.to_string()))
}

/// Parse JSON using strict DTOs. Derived DTOs reject unknown and duplicate fields.
pub fn parse_json<T: DeserializeOwned + Validate>(bytes: &[u8]) -> Result<T, ContractError> {
    preflight(bytes)?;
    let value: T =
        serde_json::from_slice(bytes).map_err(|error| ContractError::Json(error.to_string()))?;
    value.validate()?;
    Ok(value)
}

fn timestamp(value: &str, field: &'static str) -> Result<jiff::Timestamp, ContractError> {
    value
        .parse()
        .map_err(|_| invalid(field, "invalid_timestamp"))
}

fn depot_origin(value: &str, field: &'static str) -> Result<url::Url, ContractError> {
    let origin = url::Url::parse(value).map_err(|_| invalid(field, "invalid_url"))?;
    let safe_domain = match origin.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.to_ascii_lowercase();
            domain != "localhost"
                && !domain.ends_with(".localhost")
                && !domain.ends_with(".local")
                && !domain.ends_with(".internal")
        }
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) | None => false,
    };
    if origin.scheme() != "https"
        || origin.username() != ""
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !safe_domain
    {
        return Err(invalid(field, "disallowed_origin"));
    }
    Ok(origin)
}

/// Operator-approved immutable DNS resolution policy used by the eventual fetch gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedDnsPolicy {
    pub id: String,
    pub depot_origin: String,
    pub resolved_addresses: BTreeSet<IpAddr>,
}

impl Validate for ApprovedDnsPolicy {
    fn validate(&self) -> Result<(), ContractError> {
        identifier(&self.id, "dns_", "dnsPolicyId")?;
        depot_origin(&self.depot_origin, "depotOrigin")?;
        if self.resolved_addresses.is_empty()
            || self.resolved_addresses.iter().any(|address| match address {
                IpAddr::V4(address) => {
                    let [a, b, c, _] = address.octets();
                    address.is_private()
                        || address.is_loopback()
                        || address.is_link_local()
                        || address.is_multicast()
                        || address.is_unspecified()
                        || a == 0
                        || (a == 100 && (64..=127).contains(&b))
                        || (a == 192 && b == 0 && c == 0)
                        || (a == 192 && b == 0 && c == 2)
                        || (a == 198 && (b == 18 || b == 19))
                        || (a == 198 && b == 51 && c == 100)
                        || (a == 203 && b == 0 && c == 113)
                        || a >= 224
                }
                IpAddr::V6(address) => {
                    address.is_loopback()
                        || address.is_unique_local()
                        || address.is_unicast_link_local()
                        || address.is_multicast()
                        || address.is_unspecified()
                        || address.segments()[0..2] == [0x2001, 0x0db8]
                        || address.to_ipv4_mapped().is_some()
                }
            })
        {
            return Err(invalid("dnsPolicy", "disallowed_address"));
        }
        let mut hasher = Sha256::new();
        hasher.update(self.depot_origin.as_bytes());
        for address in &self.resolved_addresses {
            hasher.update(b"\n");
            hasher.update(address.to_string().as_bytes());
        }
        let mut expected = String::from("dns_");
        for byte in hasher.finalize() {
            write!(&mut expected, "{byte:02x}").expect("writing to a String cannot fail");
        }
        if self.id != expected {
            return Err(invalid("dnsPolicyId", "content_mismatch"));
        }
        Ok(())
    }
}

impl ApprovedDnsPolicy {
    /// Validate the transport's selected origin/address against this immutable policy.
    pub fn validate_selected_address(
        &self,
        origin: &str,
        address: IpAddr,
    ) -> Result<(), ContractError> {
        self.validate()?;
        if origin != self.depot_origin {
            return Err(ContractError::IdentityMismatch("depotOrigin"));
        }
        if !self.resolved_addresses.contains(&address) {
            return Err(ContractError::IdentityMismatch("resolvedAddress"));
        }
        Ok(())
    }
}

/// Parse a signed/canonical payload and require byte-for-byte canonical JSON.
pub fn parse_canonical_json<T>(bytes: &[u8]) -> Result<T, ContractError>
where
    T: DeserializeOwned + Serialize + Validate,
{
    let value = parse_json::<T>(bytes)?;
    let encoded = canonical_json::to_canonical_vec(&value)
        .map_err(|error| ContractError::Json(error.to_string()))?;
    if encoded != bytes {
        return Err(ContractError::NonCanonical);
    }
    Ok(value)
}

fn schema(value: &str, expected: &'static str) -> Result<(), ContractError> {
    if value == expected {
        Ok(())
    } else {
        Err(invalid("schemaVersion", "unsupported"))
    }
}

fn identifier(value: &str, prefix: &'static str, field: &'static str) -> Result<(), ContractError> {
    if value.len() <= prefix.len() || value.len() > 256 || !value.starts_with(prefix) {
        return Err(invalid(field, "invalid_identifier"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(field, "unsafe_character"));
    }
    Ok(())
}

fn digest(value: &str, field: &'static str) -> Result<(), ContractError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid(field, "invalid_digest"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid(field, "invalid_digest"));
    }
    Ok(())
}

fn safe_path(value: &str, absolute: bool, field: &'static str) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES || value.contains(['\\', '\0', '?', '#']) {
        return Err(invalid(field, "unsafe_path"));
    }
    if value.starts_with('/') != absolute || value.starts_with("//") || value.contains("://") {
        return Err(invalid(field, "unsafe_path"));
    }
    let relative = value.strip_prefix('/').unwrap_or(value);
    if relative
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(invalid(field, "unsafe_path"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceBinding {
    pub kind: ResourceKind,
    pub id: String,
    pub revision_id: String,
    pub content_digest: String,
}

impl Validate for ResourceBinding {
    fn validate(&self) -> Result<(), ContractError> {
        identifier(
            &self.id,
            match self.kind {
                ResourceKind::Artifact => "art_",
                ResourceKind::Loadout => "load_",
            },
            "resource.id",
        )?;
        identifier(&self.revision_id, "rev_", "resource.revisionId")?;
        digest(&self.content_digest, "resource.contentDigest")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Artifact,
    Loadout,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Reject,
    KeepExisting,
    CreateSideBySide,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RequestedOperation {
    Store,
    Materialize,
    Expose,
    Activate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryRequest {
    pub schema_version: String,
    pub delivery_handle: String,
    pub connection_id: String,
    pub target_id: String,
    pub resource: ResourceBinding,
    pub conflict_policy: ConflictPolicy,
    pub requested_operations: Vec<RequestedOperation>,
    pub idempotency_key: String,
    pub correlation_id: String,
}

impl Validate for DeliveryRequest {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, DELIVERY_SCHEMA)?;
        identifier(&self.delivery_handle, "dh_", "deliveryHandle")?;
        identifier(&self.connection_id, "con_", "connectionId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        identifier(&self.idempotency_key, "idem_", "idempotencyKey")?;
        identifier(&self.correlation_id, "cor_", "correlationId")?;
        self.resource.validate()?;
        if self.requested_operations.is_empty() || self.requested_operations.len() > 4 {
            return Err(invalid("requestedOperations", "invalid_count"));
        }
        let unique: BTreeSet<_> = self.requested_operations.iter().copied().collect();
        if unique.len() != self.requested_operations.len() {
            return Err(invalid("requestedOperations", "duplicate"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolRange {
    pub minimum: String,
    pub maximum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityLinkChallenge {
    pub schema_version: String,
    pub challenge_id: String,
    pub nonce: String,
    pub target_id: String,
    pub target_display_name: String,
    pub depot_origin: String,
    pub dns_policy_id: String,
    pub labby_key_thumbprint: String,
    pub protocol_range: ProtocolRange,
    pub expires_at: String,
}

impl Validate for IdentityLinkChallenge {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, DELIVERY_SCHEMA)?;
        identifier(&self.challenge_id, "link_", "challengeId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        if self.nonce.len() < 43 || self.nonce.len() > 128 {
            return Err(invalid("nonce", "invalid_length"));
        }
        if self.target_display_name.is_empty() || self.target_display_name.len() > 128 {
            return Err(invalid("targetDisplayName", "invalid_length"));
        }
        depot_origin(&self.depot_origin, "depotOrigin")?;
        identifier(&self.dns_policy_id, "dns_", "dnsPolicyId")?;
        digest(&self.labby_key_thumbprint, "labbyKeyThumbprint")?;
        if self.protocol_range.minimum != DELIVERY_SCHEMA
            || self.protocol_range.maximum != DELIVERY_SCHEMA
        {
            return Err(invalid("protocolRange", "unsupported"));
        }
        timestamp(&self.expires_at, "expiresAt")?;
        Ok(())
    }
}

impl IdentityLinkChallenge {
    pub fn validate_at(
        &self,
        now: jiff::Timestamp,
        skew_seconds: u64,
    ) -> Result<(), ContractError> {
        self.validate()?;
        let expires = timestamp(&self.expires_at, "expiresAt")?;
        let skew = i64::try_from(skew_seconds).map_err(|_| invalid("skew", "overflow"))?;
        if now.as_second() >= expires.as_second().saturating_add(skew) {
            return Err(invalid("expiresAt", "expired"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityLinkReceipt {
    pub schema_version: String,
    pub challenge_id: String,
    pub connection_id: String,
    pub depot_account_id: String,
    pub tenant_id: String,
    pub target_id: String,
    pub depot_origin: String,
    pub dns_policy_id: String,
    pub depot_key_thumbprint: String,
    pub labby_key_thumbprint: String,
    pub protocol_version: String,
    pub linked_at: String,
}

impl Validate for IdentityLinkReceipt {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, DELIVERY_SCHEMA)?;
        identifier(&self.challenge_id, "link_", "challengeId")?;
        identifier(&self.connection_id, "con_", "connectionId")?;
        identifier(&self.depot_account_id, "acct_", "depotAccountId")?;
        identifier(&self.tenant_id, "ten_", "tenantId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        identifier(&self.dns_policy_id, "dns_", "dnsPolicyId")?;
        digest(&self.depot_key_thumbprint, "depotKeyThumbprint")?;
        digest(&self.labby_key_thumbprint, "labbyKeyThumbprint")?;
        if self.protocol_version != DELIVERY_SCHEMA {
            return Err(invalid("protocolVersion", "unsupported"));
        }
        timestamp(&self.linked_at, "linkedAt")?;
        IdentityLinkChallenge {
            schema_version: self.schema_version.clone(),
            challenge_id: self.challenge_id.clone(),
            nonce: "x".repeat(43),
            target_id: self.target_id.clone(),
            target_display_name: "x".into(),
            depot_origin: self.depot_origin.clone(),
            dns_policy_id: self.dns_policy_id.clone(),
            labby_key_thumbprint: self.labby_key_thumbprint.clone(),
            protocol_range: ProtocolRange {
                minimum: DELIVERY_SCHEMA.into(),
                maximum: DELIVERY_SCHEMA.into(),
            },
            expires_at: "1970-01-01T00:00:00Z".into(),
        }
        .validate()
    }
}

impl IdentityLinkReceipt {
    pub fn matches_challenge(
        &self,
        challenge: &IdentityLinkChallenge,
    ) -> Result<(), ContractError> {
        if self.challenge_id != challenge.challenge_id {
            return Err(ContractError::IdentityMismatch("challengeId"));
        }
        if self.target_id != challenge.target_id {
            return Err(ContractError::IdentityMismatch("targetId"));
        }
        if self.depot_origin != challenge.depot_origin {
            return Err(ContractError::IdentityMismatch("depotOrigin"));
        }
        if self.dns_policy_id != challenge.dns_policy_id {
            return Err(ContractError::IdentityMismatch("dnsPolicyId"));
        }
        if self.labby_key_thumbprint != challenge.labby_key_thumbprint {
            return Err(ContractError::IdentityMismatch("labbyKeyThumbprint"));
        }
        Ok(())
    }

    pub fn matches_dns_policy(&self, policy: &ApprovedDnsPolicy) -> Result<(), ContractError> {
        policy.validate()?;
        if self.dns_policy_id != policy.id {
            return Err(ContractError::IdentityMismatch("dnsPolicyId"));
        }
        if self.depot_origin != policy.depot_origin {
            return Err(ContractError::IdentityMismatch("depotOrigin"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadGrantClaims {
    pub iss: String,
    pub dns_policy_id: String,
    pub sub: String,
    pub aud: String,
    pub tenant_id: String,
    pub target_id: String,
    pub connection_id: String,
    pub delivery_id: String,
    pub resource_kind: ResourceKind,
    pub resource_id: String,
    pub revision_id: String,
    pub content_digest: String,
    pub manifest_digest: String,
    pub purpose: String,
    pub protocol_version: String,
    pub artifact_schema_version: String,
    pub jti: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
}

impl Validate for DownloadGrantClaims {
    fn validate(&self) -> Result<(), ContractError> {
        identifier(&self.sub, "acct_", "sub")?;
        identifier(&self.tenant_id, "ten_", "tenantId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        identifier(&self.connection_id, "con_", "connectionId")?;
        identifier(&self.delivery_id, "del_", "deliveryId")?;
        identifier(
            &self.resource_id,
            match self.resource_kind {
                ResourceKind::Artifact => "art_",
                ResourceKind::Loadout => "load_",
            },
            "resourceId",
        )?;
        identifier(&self.revision_id, "rev_", "revisionId")?;
        identifier(&self.jti, "jti_", "jti")?;
        digest(&self.content_digest, "contentDigest")?;
        digest(&self.manifest_digest, "manifestDigest")?;
        depot_origin(&self.iss, "iss")?;
        identifier(&self.dns_policy_id, "dns_", "dnsPolicyId")?;
        if self.aud != "labby:delivery" || self.purpose != "depot-to-labby-pull" {
            return Err(invalid("grant", "wrong_audience_or_purpose"));
        }
        if self.protocol_version != DELIVERY_SCHEMA
            || self.artifact_schema_version != ARTIFACT_SCHEMA
        {
            return Err(invalid("grant", "unsupported_version"));
        }
        if self.nbf < self.iat || self.exp <= self.nbf || self.exp - self.iat > 300 {
            return Err(invalid("grant", "invalid_time_window"));
        }
        Ok(())
    }
}

impl DownloadGrantClaims {
    pub fn validate_at(
        &self,
        now: jiff::Timestamp,
        skew_seconds: u64,
    ) -> Result<(), ContractError> {
        self.validate()?;
        let now = now.as_second();
        let skew = i64::try_from(skew_seconds).map_err(|_| invalid("skew", "overflow"))?;
        let iat = i64::try_from(self.iat).map_err(|_| invalid("iat", "overflow"))?;
        let nbf = i64::try_from(self.nbf).map_err(|_| invalid("nbf", "overflow"))?;
        let exp = i64::try_from(self.exp).map_err(|_| invalid("exp", "overflow"))?;
        if iat > now.saturating_add(skew) {
            return Err(invalid("iat", "future"));
        }
        if nbf > now.saturating_add(skew) {
            return Err(invalid("nbf", "not_yet_valid"));
        }
        if exp <= now.saturating_sub(skew) {
            return Err(invalid("exp", "expired"));
        }
        Ok(())
    }

    pub fn matches_link(&self, link: &IdentityLinkReceipt) -> Result<(), ContractError> {
        for (field, matches) in [
            ("iss", self.iss == link.depot_origin),
            ("dnsPolicyId", self.dns_policy_id == link.dns_policy_id),
            ("sub", self.sub == link.depot_account_id),
            ("tenantId", self.tenant_id == link.tenant_id),
            ("targetId", self.target_id == link.target_id),
            ("connectionId", self.connection_id == link.connection_id),
        ] {
            if !matches {
                return Err(ContractError::IdentityMismatch(field));
            }
        }
        Ok(())
    }

    pub fn matches_dns_policy(&self, policy: &ApprovedDnsPolicy) -> Result<(), ContractError> {
        policy.validate()?;
        if self.dns_policy_id != policy.id {
            return Err(ContractError::IdentityMismatch("dnsPolicyId"));
        }
        if self.iss != policy.depot_origin {
            return Err(ContractError::IdentityMismatch("iss"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkManifest {
    pub schema_version: String,
    pub delivery_id: String,
    pub target_id: String,
    pub revision_id: String,
    pub content_digest: String,
    pub total_compressed_bytes: u64,
    pub total_uncompressed_bytes: u64,
    pub components: Vec<ManifestComponent>,
    pub chunks: Vec<ManifestChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestComponent {
    pub component_id: String,
    pub path: String,
    pub dependencies: Vec<String>,
    pub chunks: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestChunk {
    pub ordinal: u32,
    pub bytes: u64,
    pub digest: String,
    pub download_path: String,
}

impl Validate for ChunkManifest {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, MANIFEST_SCHEMA)?;
        identifier(&self.delivery_id, "del_", "deliveryId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        identifier(&self.revision_id, "rev_", "revisionId")?;
        digest(&self.content_digest, "contentDigest")?;
        if self.components.is_empty() || self.components.len() > MAX_COMPONENTS {
            return Err(invalid("components", "invalid_count"));
        }
        if self.chunks.is_empty() || self.chunks.len() > MAX_CHUNKS {
            return Err(invalid("chunks", "invalid_count"));
        }
        if self.total_compressed_bytes > MAX_COMPRESSED_BYTES
            || self.total_uncompressed_bytes > MAX_UNCOMPRESSED_BYTES
            || self.total_uncompressed_bytes > self.total_compressed_bytes.saturating_mul(20)
        {
            return Err(invalid("bytes", "limit_exceeded"));
        }
        let mut chunk_total = 0_u64;
        let mut chunk_ids = BTreeSet::new();
        for (index, chunk) in self.chunks.iter().enumerate() {
            if chunk.ordinal as usize != index
                || !chunk_ids.insert(chunk.ordinal)
                || chunk.bytes > MAX_CHUNK_BYTES
            {
                return Err(invalid("chunks", "invalid_ordinal_or_size"));
            }
            digest(&chunk.digest, "chunk.digest")?;
            safe_path(&chunk.download_path, true, "chunk.downloadPath")?;
            let expected = format!(
                "/v1/deliveries/{}/chunks/{}",
                self.delivery_id, chunk.ordinal
            );
            if chunk.download_path != expected {
                return Err(invalid("chunk.downloadPath", "binding_mismatch"));
            }
            chunk_total = chunk_total
                .checked_add(chunk.bytes)
                .ok_or_else(|| invalid("chunks", "byte_overflow"))?;
        }
        if chunk_total != self.total_compressed_bytes {
            return Err(invalid("totalCompressedBytes", "mismatch"));
        }
        let ids: BTreeSet<_> = self
            .components
            .iter()
            .map(|item| item.component_id.as_str())
            .collect();
        if ids.len() != self.components.len() {
            return Err(invalid("components", "duplicate_id"));
        }
        let mut paths = BTreeSet::new();
        let mut edges = 0_usize;
        let mut graph = BTreeMap::new();
        for component in &self.components {
            identifier(&component.component_id, "cmp_", "componentId")?;
            safe_path(&component.path, false, "component.path")?;
            if !paths.insert(component.path.to_ascii_lowercase()) {
                return Err(invalid("components", "duplicate_path"));
            }
            edges = edges
                .checked_add(component.dependencies.len())
                .ok_or_else(|| invalid("dependencies", "overflow"))?;
            if edges > MAX_EDGES
                || component
                    .dependencies
                    .iter()
                    .any(|dependency| !ids.contains(dependency.as_str()))
            {
                return Err(invalid("dependencies", "invalid_graph"));
            }
            if component.chunks.is_empty()
                || component
                    .chunks
                    .iter()
                    .any(|ordinal| !chunk_ids.contains(ordinal))
            {
                return Err(invalid("component.chunks", "unknown_chunk"));
            }
            graph.insert(
                component.component_id.as_str(),
                component
                    .dependencies
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
        }
        fn visit<'a>(
            node: &'a str,
            graph: &BTreeMap<&'a str, Vec<&'a str>>,
            active: &mut BTreeSet<&'a str>,
            done: &mut BTreeSet<&'a str>,
            depth: usize,
        ) -> Result<(), ContractError> {
            if depth > MAX_DEPTH {
                return Err(invalid("dependencies", "depth_exceeded"));
            }
            if done.contains(node) {
                return Ok(());
            }
            if !active.insert(node) {
                return Err(invalid("dependencies", "cycle"));
            }
            for child in &graph[node] {
                visit(child, graph, active, done, depth + 1)?;
            }
            active.remove(node);
            done.insert(node);
            Ok(())
        }
        let mut done = BTreeSet::new();
        for node in graph.keys() {
            visit(node, &graph, &mut BTreeSet::new(), &mut done, 1)?;
        }
        Ok(())
    }
}

impl DownloadGrantClaims {
    pub fn matches_request(&self, request: &DeliveryRequest) -> Result<(), ContractError> {
        for (field, matches) in [
            ("targetId", self.target_id == request.target_id),
            ("connectionId", self.connection_id == request.connection_id),
            ("resourceKind", self.resource_kind == request.resource.kind),
            ("resourceId", self.resource_id == request.resource.id),
            (
                "revisionId",
                self.revision_id == request.resource.revision_id,
            ),
            (
                "contentDigest",
                self.content_digest == request.resource.content_digest,
            ),
        ] {
            if !matches {
                return Err(ContractError::IdentityMismatch(field));
            }
        }
        Ok(())
    }

    pub fn matches_manifest(&self, manifest: &ChunkManifest) -> Result<(), ContractError> {
        for (field, matches) in [
            ("deliveryId", self.delivery_id == manifest.delivery_id),
            ("targetId", self.target_id == manifest.target_id),
            ("revisionId", self.revision_id == manifest.revision_id),
            (
                "contentDigest",
                self.content_digest == manifest.content_digest,
            ),
        ] {
            if !matches {
                return Err(ContractError::IdentityMismatch(field));
            }
        }
        let digest = canonical_json::digest(manifest)
            .map_err(|error| ContractError::Json(error.to_string()))?;
        if digest != self.manifest_digest {
            return Err(ContractError::IdentityMismatch("manifestDigest"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Requested,
    Granted,
    Transferred,
    Verified,
    Stored,
    Materialized,
    Exposed,
    Activated,
    Incompatible,
    Partial,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryReceipt {
    pub schema_version: String,
    pub receipt_id: String,
    pub sequence: u64,
    pub delivery_id: String,
    pub correlation_id: String,
    pub connection_id: String,
    pub tenant_id: String,
    pub target_id: String,
    pub resource: ResourceBinding,
    pub state: DeliveryState,
    pub components: Vec<ComponentReceipt>,
    pub summary: ReceiptSummary,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentReceipt {
    pub component_id: String,
    pub state: DeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_through: Option<DeliveryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_transferred: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptSummary {
    pub requested: u32,
    pub granted: u32,
    pub transferred: u32,
    pub verified: u32,
    pub stored: u32,
    pub materialized: u32,
    pub exposed: u32,
    pub activated: u32,
    pub incompatible: u32,
    pub partial: u32,
    pub cancelled: u32,
    pub failed: u32,
}

fn success_rank(state: DeliveryState) -> Option<u8> {
    match state {
        DeliveryState::Requested => Some(0),
        DeliveryState::Granted => Some(1),
        DeliveryState::Transferred => Some(2),
        DeliveryState::Verified => Some(3),
        DeliveryState::Stored => Some(4),
        DeliveryState::Materialized => Some(5),
        DeliveryState::Exposed => Some(6),
        DeliveryState::Activated => Some(7),
        DeliveryState::Partial
        | DeliveryState::Incompatible
        | DeliveryState::Cancelled
        | DeliveryState::Failed => None,
    }
}

fn component_milestone(component: &ComponentReceipt) -> Result<u8, ContractError> {
    let rank = match component.state {
        DeliveryState::Requested => 0,
        DeliveryState::Granted => 1,
        DeliveryState::Transferred => 2,
        DeliveryState::Verified => 3,
        DeliveryState::Stored => 4,
        DeliveryState::Materialized => 5,
        DeliveryState::Exposed => 6,
        DeliveryState::Activated => 7,
        DeliveryState::Partial => component
            .completed_through
            .and_then(success_rank)
            .filter(|rank| *rank < 7)
            .ok_or_else(|| invalid("component.completedThrough", "required_for_partial"))?,
        DeliveryState::Incompatible | DeliveryState::Cancelled | DeliveryState::Failed => {
            match component.stage.as_deref() {
                Some("grant") => 0,
                Some("transfer") => 1,
                Some("verification") => 2,
                Some("storage") => 3,
                Some("materialization") => 4,
                Some("exposure" | "activation") => 5,
                _ => return Err(invalid("component.stage", "unsupported")),
            }
        }
    };
    Ok(rank)
}

fn derived_summary(components: &[ComponentReceipt]) -> Result<ReceiptSummary, ContractError> {
    let mut summary = ReceiptSummary {
        requested: components.len() as u32,
        granted: 0,
        transferred: 0,
        verified: 0,
        stored: 0,
        materialized: 0,
        exposed: 0,
        activated: 0,
        incompatible: 0,
        partial: 0,
        cancelled: 0,
        failed: 0,
    };
    for component in components {
        let milestone = component_milestone(component)?;
        summary.granted += u32::from(milestone >= 1);
        summary.transferred += u32::from(milestone >= 2);
        summary.verified += u32::from(milestone >= 3);
        summary.stored += u32::from(milestone >= 4);
        summary.materialized += u32::from(milestone >= 5);
        summary.exposed += u32::from(milestone >= 6);
        summary.activated += u32::from(milestone >= 7);
        summary.incompatible += u32::from(component.state == DeliveryState::Incompatible);
        summary.cancelled += u32::from(component.state == DeliveryState::Cancelled);
        summary.failed += u32::from(component.state == DeliveryState::Failed);
        summary.partial += u32::from(matches!(
            component.state,
            DeliveryState::Partial
                | DeliveryState::Incompatible
                | DeliveryState::Cancelled
                | DeliveryState::Failed
        ));
    }
    Ok(summary)
}

impl Validate for DeliveryReceipt {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, DELIVERY_SCHEMA)?;
        identifier(&self.receipt_id, "rcpt_", "receiptId")?;
        if self.sequence == 0 {
            return Err(invalid("sequence", "zero"));
        }
        identifier(&self.delivery_id, "del_", "deliveryId")?;
        identifier(&self.correlation_id, "cor_", "correlationId")?;
        identifier(&self.connection_id, "con_", "connectionId")?;
        identifier(&self.tenant_id, "ten_", "tenantId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        self.resource.validate()?;
        if self.components.is_empty() || self.components.len() > MAX_COMPONENTS {
            return Err(invalid("components", "invalid_count"));
        }
        let mut ids = BTreeSet::new();
        for component in &self.components {
            identifier(&component.component_id, "cmp_", "componentId")?;
            if !ids.insert(&component.component_id) {
                return Err(invalid("components", "duplicate_id"));
            }
            if matches!(
                component.state,
                DeliveryState::Failed | DeliveryState::Incompatible
            ) && (component.stage.as_deref().is_none_or(str::is_empty)
                || component.code.as_deref().is_none_or(str::is_empty)
                || component.retryable.is_none())
            {
                return Err(invalid("component", "missing_failure_detail"));
            }
            if component.state != DeliveryState::Partial && component.completed_through.is_some() {
                return Err(invalid(
                    "component.completedThrough",
                    "only_valid_for_partial",
                ));
            }
            if component
                .message
                .as_ref()
                .is_some_and(|message| message.len() > 512)
            {
                return Err(invalid("component.message", "too_long"));
            }
        }
        if self.summary != derived_summary(&self.components)? {
            return Err(invalid("summary", "component_mismatch"));
        }
        let count = self.components.len() as u32;
        for (field, value) in [
            ("requested", self.summary.requested),
            ("granted", self.summary.granted),
            ("transferred", self.summary.transferred),
            ("verified", self.summary.verified),
            ("stored", self.summary.stored),
            ("materialized", self.summary.materialized),
            ("exposed", self.summary.exposed),
            ("activated", self.summary.activated),
            ("incompatible", self.summary.incompatible),
            ("partial", self.summary.partial),
            ("cancelled", self.summary.cancelled),
            ("failed", self.summary.failed),
        ] {
            if value > count {
                return Err(invalid(field, "count_exceeds_components"));
            }
        }
        timestamp(&self.occurred_at, "occurredAt")?;
        if self.summary.requested != count {
            return Err(invalid("summary.requested", "must_equal_components"));
        }
        let ordered = [
            self.summary.requested,
            self.summary.granted,
            self.summary.transferred,
            self.summary.verified,
            self.summary.stored,
            self.summary.materialized,
            self.summary.exposed,
            self.summary.activated,
        ];
        if ordered.windows(2).any(|pair| pair[1] > pair[0]) {
            return Err(invalid("summary", "nonmonotonic_counts"));
        }
        let terminal = self
            .summary
            .incompatible
            .checked_add(self.summary.cancelled)
            .and_then(|v| v.checked_add(self.summary.failed))
            .ok_or_else(|| invalid("summary", "overflow"))?;
        if terminal > count || self.summary.partial > count {
            return Err(invalid("summary", "terminal_count_exceeded"));
        }
        match self.state {
            DeliveryState::Requested if self.summary.granted != 0 => {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Granted
                if self.summary.granted != count || self.summary.transferred == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Transferred
                if self.summary.transferred != count || self.summary.verified == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Verified
                if self.summary.verified != count || self.summary.stored == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Stored
                if self.summary.stored != count || self.summary.materialized == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Materialized
                if self.summary.materialized != count
                    || self.summary.exposed == count
                    || self.summary.activated == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Exposed
                if self.summary.exposed != count || self.summary.activated == count =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Activated
                if self.summary.activated != count
                    || self
                        .components
                        .iter()
                        .any(|c| c.state != DeliveryState::Activated) =>
            {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Partial if self.summary.partial == 0 && terminal == 0 => {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Failed if self.summary.failed != count => {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Cancelled if self.summary.cancelled != count => {
                return Err(invalid("state", "summary_mismatch"));
            }
            DeliveryState::Incompatible if self.summary.incompatible != count => {
                return Err(invalid("state", "summary_mismatch"));
            }
            _ => {}
        }
        Ok(())
    }
}

impl DeliveryReceipt {
    pub fn matches_manifest(&self, manifest: &ChunkManifest) -> Result<(), ContractError> {
        self.validate()?;
        manifest.validate()?;
        for (field, matches) in [
            ("deliveryId", self.delivery_id == manifest.delivery_id),
            ("targetId", self.target_id == manifest.target_id),
            (
                "revisionId",
                self.resource.revision_id == manifest.revision_id,
            ),
            (
                "contentDigest",
                self.resource.content_digest == manifest.content_digest,
            ),
        ] {
            if !matches {
                return Err(ContractError::IdentityMismatch(field));
            }
        }
        let receipt_ids: BTreeSet<_> = self
            .components
            .iter()
            .map(|component| component.component_id.as_str())
            .collect();
        let manifest_ids: BTreeSet<_> = manifest
            .components
            .iter()
            .map(|component| component.component_id.as_str())
            .collect();
        if receipt_ids != manifest_ids {
            return Err(invalid("components", "manifest_mismatch"));
        }
        Ok(())
    }

    pub fn follows(&self, previous: &Self) -> Result<(), ContractError> {
        self.validate()?;
        previous.validate()?;
        for (field, matches) in [
            ("deliveryId", self.delivery_id == previous.delivery_id),
            (
                "correlationId",
                self.correlation_id == previous.correlation_id,
            ),
            ("connectionId", self.connection_id == previous.connection_id),
            ("tenantId", self.tenant_id == previous.tenant_id),
            ("targetId", self.target_id == previous.target_id),
            ("resource", self.resource == previous.resource),
        ] {
            if !matches {
                return Err(ContractError::IdentityMismatch(field));
            }
        }
        if self.sequence <= previous.sequence {
            return Err(invalid("sequence", "regression"));
        }
        if self.receipt_id == previous.receipt_id {
            return Err(invalid("receiptId", "reused"));
        }
        if timestamp(&self.occurred_at, "occurredAt")?
            < timestamp(&previous.occurred_at, "occurredAt")?
        {
            return Err(invalid("occurredAt", "regression"));
        }
        if matches!(
            previous.state,
            DeliveryState::Activated
                | DeliveryState::Incompatible
                | DeliveryState::Cancelled
                | DeliveryState::Failed
        ) {
            return Err(invalid("state", "terminal_transition"));
        }
        if !allowed_receipt_transition(previous.state, self.state) {
            return Err(invalid("state", "invalid_transition"));
        }
        for (prior, next) in [
            (previous.summary.requested, self.summary.requested),
            (previous.summary.granted, self.summary.granted),
            (previous.summary.transferred, self.summary.transferred),
            (previous.summary.verified, self.summary.verified),
            (previous.summary.stored, self.summary.stored),
            (previous.summary.materialized, self.summary.materialized),
            (previous.summary.exposed, self.summary.exposed),
            (previous.summary.activated, self.summary.activated),
            (previous.summary.incompatible, self.summary.incompatible),
            (previous.summary.cancelled, self.summary.cancelled),
            (previous.summary.failed, self.summary.failed),
        ] {
            if next < prior {
                return Err(invalid("summary", "regression"));
            }
        }
        let current: BTreeMap<_, _> = self
            .components
            .iter()
            .map(|component| (&component.component_id, component))
            .collect();
        let previous_ids: BTreeSet<_> = previous
            .components
            .iter()
            .map(|component| component.component_id.as_str())
            .collect();
        let current_ids: BTreeSet<_> = current.keys().map(|id| id.as_str()).collect();
        if current_ids != previous_ids {
            return Err(invalid("components", "set_mismatch"));
        }
        for prior in &previous.components {
            let Some(next) = current.get(&prior.component_id) else {
                return Err(invalid("components", "missing_previous"));
            };
            if !allowed_component_transition(prior, next)? {
                return Err(invalid("component.state", "invalid_transition"));
            }
        }
        Ok(())
    }
}

fn success_successor(previous: DeliveryState, next: DeliveryState) -> bool {
    match previous {
        DeliveryState::Requested => next == DeliveryState::Granted,
        DeliveryState::Granted => next == DeliveryState::Transferred,
        DeliveryState::Transferred => next == DeliveryState::Verified,
        DeliveryState::Verified => next == DeliveryState::Stored,
        DeliveryState::Stored => {
            matches!(next, DeliveryState::Materialized | DeliveryState::Exposed)
        }
        DeliveryState::Materialized => {
            matches!(next, DeliveryState::Exposed | DeliveryState::Activated)
        }
        DeliveryState::Exposed => next == DeliveryState::Activated,
        _ => false,
    }
}

fn allowed_component_transition(
    previous: &ComponentReceipt,
    next: &ComponentReceipt,
) -> Result<bool, ContractError> {
    if matches!(
        previous.state,
        DeliveryState::Activated
            | DeliveryState::Incompatible
            | DeliveryState::Cancelled
            | DeliveryState::Failed
    ) {
        return Ok(next == previous);
    }
    let prior_milestone = component_milestone(previous)?;
    let next_milestone = component_milestone(next)?;
    if matches!(
        next.state,
        DeliveryState::Incompatible | DeliveryState::Cancelled | DeliveryState::Failed
    ) {
        return Ok(next_milestone == prior_milestone);
    }
    if next.state == DeliveryState::Partial {
        return Ok(next_milestone == prior_milestone);
    }
    let prior_success = if previous.state == DeliveryState::Partial {
        previous.completed_through
    } else {
        Some(previous.state)
    };
    Ok(prior_success
        .is_some_and(|state| next.state == state || success_successor(state, next.state)))
}

fn allowed_receipt_transition(previous: DeliveryState, next: DeliveryState) -> bool {
    if previous == next {
        return true;
    }
    if matches!(
        next,
        DeliveryState::Partial
            | DeliveryState::Incompatible
            | DeliveryState::Cancelled
            | DeliveryState::Failed
    ) {
        return true;
    }
    match previous {
        DeliveryState::Requested => next == DeliveryState::Granted,
        DeliveryState::Granted => next == DeliveryState::Transferred,
        DeliveryState::Transferred => next == DeliveryState::Verified,
        DeliveryState::Verified => next == DeliveryState::Stored,
        DeliveryState::Stored => {
            matches!(next, DeliveryState::Materialized | DeliveryState::Exposed)
        }
        DeliveryState::Materialized => {
            matches!(next, DeliveryState::Exposed | DeliveryState::Activated)
        }
        DeliveryState::Exposed => next == DeliveryState::Activated,
        DeliveryState::Partial => !matches!(next, DeliveryState::Requested),
        DeliveryState::Activated
        | DeliveryState::Incompatible
        | DeliveryState::Cancelled
        | DeliveryState::Failed => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryErrorEnvelope {
    pub schema_version: String,
    pub error: DeliveryError,
    pub delivery_id: String,
    pub correlation_id: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryError {
    pub code: String,
    pub stage: String,
    pub retryable: bool,
    pub message: String,
}

impl Validate for DeliveryErrorEnvelope {
    fn validate(&self) -> Result<(), ContractError> {
        schema(&self.schema_version, DELIVERY_SCHEMA)?;
        identifier(&self.delivery_id, "del_", "deliveryId")?;
        identifier(&self.correlation_id, "cor_", "correlationId")?;
        identifier(&self.target_id, "labby_", "targetId")?;
        for (field, value) in [
            ("error.code", &self.error.code),
            ("error.stage", &self.error.stage),
        ] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(invalid(field, "unsafe_value"));
            }
        }
        if self.error.message.is_empty()
            || self.error.message.len() > 512
            || self.error.message.contains("://")
        {
            return Err(invalid("error.message", "unsafe_value"));
        }
        Ok(())
    }
}

/// Reject recursively unbounded or secret-shaped extension data before future DTOs accept it.
pub fn validate_extension(value: &Value) -> Result<(), ContractError> {
    fn walk(value: &Value, depth: usize) -> Result<(), ContractError> {
        if depth > 8 {
            return Err(invalid("extension", "depth_exceeded"));
        }
        match value {
            Value::Object(map) => {
                if map.len() > 128 {
                    return Err(invalid("extension", "map_too_large"));
                }
                for (key, child) in map {
                    let key_lower = key.to_ascii_lowercase();
                    if [
                        "authorization",
                        "credential",
                        "password",
                        "secret",
                        "token",
                        "grant",
                    ]
                    .iter()
                    .any(|part| key_lower.contains(part))
                    {
                        return Err(invalid("extension", "secret_shaped_key"));
                    }
                    walk(child, depth + 1)?;
                }
            }
            Value::Array(values) => {
                if values.len() > 256 {
                    return Err(invalid("extension", "list_too_large"));
                }
                for child in values {
                    walk(child, depth + 1)?;
                }
            }
            Value::String(value) if value.len() > 16_384 => {
                return Err(invalid("extension", "string_too_large"));
            }
            _ => {}
        }
        Ok(())
    }
    walk(value, 0)
}
