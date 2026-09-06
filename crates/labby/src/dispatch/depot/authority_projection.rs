//! Asynchronous, bounded Labby authority projection into Depot.
#![allow(
    dead_code,
    reason = "constructed by optional Depot projection startup wiring"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use labby_primitives::authority_projection::{
    AUTHORITY_PROJECTION_SCHEMA_VERSION, AuthorityProjectionAck, AuthorityProjectionEnvelope,
    AuthorityProjectionRecord, MAX_AUTHORITY_ENVELOPE_BYTES, MAX_AUTHORITY_RECORDS_PER_BATCH,
    ProjectionKind,
};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::access::AccessStore;

const SEND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionSendError {
    #[error("authority projection configuration is invalid")]
    Configuration,
    #[error("authority projection store is unavailable")]
    Store,
    #[error("authority projection transport is unavailable")]
    Transport,
    #[error("authority projection was rejected")]
    Rejected,
    #[error("authority projection response is invalid")]
    InvalidResponse,
}

#[derive(Clone)]
pub(crate) struct AuthorityProjectionSender {
    http: Client,
    endpoint: Url,
    readiness_endpoint: Url,
    bearer: Arc<str>,
    installation_id: Arc<str>,
    key_id: Arc<str>,
    signing_key: Arc<SigningKey>,
    store: AccessStore,
}

impl std::fmt::Debug for AuthorityProjectionSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityProjectionSender")
            .field("endpoint", &self.endpoint)
            .field("installation_id", &self.installation_id)
            .field("key_id", &self.key_id)
            .field("credentials", &"<redacted>")
            .finish()
    }
}

impl AuthorityProjectionSender {
    pub(crate) fn new(
        base_url: Url,
        bearer: impl Into<Arc<str>>,
        installation_id: impl Into<Arc<str>>,
        key_id: impl Into<Arc<str>>,
        secret_key: [u8; 32],
        store: AccessStore,
    ) -> Result<Self, ProjectionSendError> {
        let endpoint = base_url
            .join("/api/authority/projection")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let readiness_endpoint = base_url
            .join("/api/authority/readiness")
            .map_err(|_| ProjectionSendError::Configuration)?;
        let bearer = bearer.into();
        let installation_id = installation_id.into();
        let key_id = key_id.into();
        if bearer.is_empty() || installation_id.is_empty() || key_id.is_empty() {
            return Err(ProjectionSendError::Configuration);
        }
        Ok(Self {
            http: Client::builder()
                .timeout(SEND_TIMEOUT)
                .build()
                .map_err(|_| ProjectionSendError::Configuration)?,
            endpoint,
            readiness_endpoint,
            bearer,
            installation_id,
            key_id,
            signing_key: Arc::new(SigningKey::from_bytes(&secret_key)),
            store,
        })
    }

    pub(crate) async fn readiness(&self) -> Result<ProjectionReadiness, ProjectionSendError> {
        let response = self
            .http
            .get(self.readiness_endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let readiness: ProjectionReadiness = response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)?;
        if !readiness.ready {
            return Err(ProjectionSendError::Rejected);
        }
        Ok(readiness)
    }

    /// Sends a caller-generated complete organization snapshot after binding it
    /// to Depot's durable watermark. A restored/older producer cannot roll the
    /// consumer backward because sequence and previous digest come from readiness.
    pub(crate) async fn send_snapshot(
        &self,
        organization_id: &str,
        mut records: Vec<AuthorityProjectionRecord>,
        now: i64,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        if records.is_empty() || records.len() > MAX_AUTHORITY_RECORDS_PER_BATCH {
            return Err(ProjectionSendError::Configuration);
        }
        let readiness = self.readiness().await?;
        let watermark = readiness.organizations.get(organization_id);
        let base = watermark.map_or(0, |value| value.highest_contiguous_sequence);
        for (offset, record) in records.iter_mut().enumerate() {
            record.sequence = base
                .checked_add(
                    u64::try_from(offset).map_err(|_| ProjectionSendError::Configuration)? + 1,
                )
                .ok_or(ProjectionSendError::Configuration)?;
        }
        let envelope = sign_envelope(
            self.installation_id.as_ref(),
            organization_id,
            ProjectionKind::Snapshot,
            Some(base),
            now.to_string(),
            watermark.and_then(|value| value.last_envelope_digest.clone()),
            self.key_id.as_ref(),
            records,
            self.signing_key.as_ref(),
        )?;
        let body = serde_json::to_vec(&envelope).map_err(|_| ProjectionSendError::Configuration)?;
        if body.len() > MAX_AUTHORITY_ENVELOPE_BYTES {
            return Err(ProjectionSendError::Configuration);
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        let ack: AuthorityProjectionAck = response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)?;
        if ack.organization_id != organization_id || ack.highest_contiguous_sequence <= base {
            return Err(ProjectionSendError::InvalidResponse);
        }
        Ok(ack)
    }

    /// Performs at most one bounded delivery pass. It is intended for a supervised
    /// background loop; authorization and mutation responses never await this method.
    pub(crate) async fn send_once(&self, now: i64) -> Result<usize, ProjectionSendError> {
        let pending = self
            .store
            .claim_authority_projection_batch(now, MAX_AUTHORITY_RECORDS_PER_BATCH)
            .await
            .map_err(|_| ProjectionSendError::Store)?;
        if pending.is_empty() {
            return Ok(0);
        }
        let mut organizations: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for row in pending {
            organizations
                .entry(row.organization_id.clone())
                .or_default()
                .push(row);
        }
        let mut sent = 0;
        for (organization_id, rows) in organizations {
            let through = rows.last().map(|row| row.sequence).unwrap_or_default();
            let result = self.send_delta(&organization_id, &rows, now).await;
            match result {
                Ok(ack)
                    if ack.organization_id == organization_id
                        && ack.highest_contiguous_sequence == through =>
                {
                    self.store
                        .acknowledge_authority_projection(
                            organization_id,
                            through,
                            ack.last_envelope_digest,
                            now,
                        )
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    sent += rows.len();
                }
                Ok(_) => {
                    self.store
                        .release_failed_authority_projection(organization_id, through, now)
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    return Err(ProjectionSendError::InvalidResponse);
                }
                Err(error) => {
                    self.store
                        .release_failed_authority_projection(organization_id, through, now)
                        .await
                        .map_err(|_| ProjectionSendError::Store)?;
                    return Err(error);
                }
            }
        }
        Ok(sent)
    }

    async fn send_delta(
        &self,
        organization_id: &str,
        rows: &[crate::access::PendingProjection],
        now: i64,
    ) -> Result<AuthorityProjectionAck, ProjectionSendError> {
        let records = rows
            .iter()
            .map(|row| {
                let value: Value = serde_json::from_str(&row.payload_json)
                    .map_err(|_| ProjectionSendError::Store)?;
                Ok(AuthorityProjectionRecord {
                    sequence: row.sequence,
                    resource_type: value
                        .get("resource_type")
                        .and_then(Value::as_str)
                        .ok_or(ProjectionSendError::Store)?
                        .to_owned(),
                    resource_id: value
                        .get("resource_id")
                        .and_then(Value::as_str)
                        .ok_or(ProjectionSendError::Store)?
                        .to_owned(),
                    operation: value
                        .get("operation")
                        .and_then(Value::as_str)
                        .ok_or(ProjectionSendError::Store)?
                        .to_owned(),
                    value: value.get("value").cloned().filter(|value| !value.is_null()),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let previous_digest = rows.first().and_then(|row| row.previous_digest.clone());
        let generated_at = now.to_string();
        let envelope = sign_envelope(
            self.installation_id.as_ref(),
            organization_id,
            ProjectionKind::Delta,
            None,
            generated_at,
            previous_digest,
            self.key_id.as_ref(),
            records,
            self.signing_key.as_ref(),
        )?;
        let body = serde_json::to_vec(&envelope).map_err(|_| ProjectionSendError::Configuration)?;
        if body.len() > MAX_AUTHORITY_ENVELOPE_BYTES {
            return Err(ProjectionSendError::Configuration);
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(self.bearer.as_ref())
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ProjectionSendError::Transport)?;
        if response.status() != StatusCode::OK {
            return Err(ProjectionSendError::Rejected);
        }
        response
            .json()
            .await
            .map_err(|_| ProjectionSendError::InvalidResponse)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionReadiness {
    pub(crate) ready: bool,
    #[serde(default)]
    pub(crate) organizations: BTreeMap<String, ProjectionWatermark>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ProjectionWatermark {
    pub(crate) highest_contiguous_sequence: u64,
    pub(crate) last_envelope_digest: Option<String>,
}

#[derive(Serialize)]
struct UnsignedEnvelope<'a> {
    schema_version: u16,
    installation_id: &'a str,
    organization_id: &'a str,
    sequence_start: u64,
    sequence_end: u64,
    kind: ProjectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_base_sequence: Option<u64>,
    generated_at: &'a str,
    previous_digest: &'a Option<String>,
    payload_digest: &'a str,
    key_id: &'a str,
    records: &'a [AuthorityProjectionRecord],
}

fn sign_envelope(
    installation_id: &str,
    organization_id: &str,
    kind: ProjectionKind,
    snapshot_base_sequence: Option<u64>,
    generated_at: String,
    previous_digest: Option<String>,
    key_id: &str,
    records: Vec<AuthorityProjectionRecord>,
    key: &SigningKey,
) -> Result<AuthorityProjectionEnvelope, ProjectionSendError> {
    let sequence_start = records
        .first()
        .map(|r| r.sequence)
        .ok_or(ProjectionSendError::Configuration)?;
    let sequence_end = records
        .last()
        .map(|r| r.sequence)
        .ok_or(ProjectionSendError::Configuration)?;
    let records_bytes = canonical_json(
        &serde_json::to_value(&records).map_err(|_| ProjectionSendError::Configuration)?,
    )?;
    let payload_digest = format!("sha256:{}", hex::encode(Sha256::digest(&records_bytes)));
    let unsigned = UnsignedEnvelope {
        schema_version: AUTHORITY_PROJECTION_SCHEMA_VERSION,
        installation_id,
        organization_id,
        sequence_start,
        sequence_end,
        kind,
        snapshot_base_sequence,
        generated_at: &generated_at,
        previous_digest: &previous_digest,
        payload_digest: &payload_digest,
        key_id,
        records: &records,
    };
    let signing_bytes = canonical_json(
        &serde_json::to_value(unsigned).map_err(|_| ProjectionSendError::Configuration)?,
    )?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(key.sign(&signing_bytes).to_bytes());
    Ok(AuthorityProjectionEnvelope {
        schema_version: AUTHORITY_PROJECTION_SCHEMA_VERSION,
        installation_id: installation_id.into(),
        organization_id: organization_id.into(),
        sequence_start,
        sequence_end,
        kind,
        snapshot_base_sequence,
        generated_at,
        previous_digest,
        payload_digest,
        key_id: key_id.into(),
        records,
        signature,
    })
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, ProjectionSendError> {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), normalize(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(normalize).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&normalize(value)).map_err(|_| ProjectionSendError::Configuration)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_signing_is_stable_and_omits_signature() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let record = AuthorityProjectionRecord {
            sequence: 1,
            resource_type: "team".into(),
            resource_id: "t1".into(),
            operation: "upsert".into(),
            value: Some(serde_json::json!({"z":1,"a":2})),
        };
        let one = sign_envelope(
            "install",
            "org",
            ProjectionKind::Delta,
            None,
            "2026-09-05T00:00:00Z".into(),
            None,
            "key",
            vec![record.clone()],
            &key,
        )
        .unwrap();
        let two = sign_envelope(
            "install",
            "org",
            ProjectionKind::Delta,
            None,
            "2026-09-05T00:00:00Z".into(),
            None,
            "key",
            vec![record],
            &key,
        )
        .unwrap();
        assert_eq!(one, two);
        assert!(one.payload_digest.starts_with("sha256:"));
        assert!(!one.signature.contains('='));
    }

    #[test]
    fn canonical_json_matches_depot_golden_fixture() {
        let value = serde_json::json!({"sequence_start":1,"records":[],"previous_digest":null,"payload_digest":"sha256:placeholder","organization_id":"org-1","kind":"delta","key_id":"key-1","installation_id":"install-1","generated_at":"2026-09-05T00:00:00Z","sequence_end":1,"schema_version":1});
        assert_eq!(
            String::from_utf8(canonical_json(&value).unwrap()).unwrap(),
            "{\"generated_at\":\"2026-09-05T00:00:00Z\",\"installation_id\":\"install-1\",\"key_id\":\"key-1\",\"kind\":\"delta\",\"organization_id\":\"org-1\",\"payload_digest\":\"sha256:placeholder\",\"previous_digest\":null,\"records\":[],\"schema_version\":1,\"sequence_end\":1,\"sequence_start\":1}"
        );
    }
}
