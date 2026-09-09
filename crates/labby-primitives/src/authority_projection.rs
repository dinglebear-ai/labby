//! Versioned, bounded wire contract for Labby authority projection into Depot.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digest::Sha256Digest;

pub const AUTHORITY_PROJECTION_SCHEMA_VERSION: u16 = 1;
pub const MAX_AUTHORITY_RECORDS_PER_BATCH: usize = 256;
pub const MAX_AUTHORITY_ENVELOPE_BYTES: usize = 512 * 1024;
/// Producers send a heartbeat at least this often while the outbox is idle so
/// Depot can distinguish a quiet authority from a dead one.
pub const AUTHORITY_HEARTBEAT_INTERVAL_SECONDS: u64 = 60;

/// Envelope kind. Snapshot metadata lives inside the variant so a snapshot
/// envelope can never be missing its identity or completion flag.
///
/// The wire shape is unchanged: `kind` is the discriminator and the snapshot
/// fields stay top-level (`snapshot_id`, `snapshot_base_sequence`,
/// `snapshot_complete`). Labby v1 emits only `snapshot` and `heartbeat`;
/// `delta` remains part of the accepted vocabulary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionKind {
    Delta,
    Snapshot {
        /// Stable identifier shared by every chunk of one staged snapshot.
        snapshot_id: String,
        /// Watermark the snapshot replaces; absent on continuation chunks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot_base_sequence: Option<u64>,
        /// Activates the staged snapshot after this envelope is durably accepted.
        snapshot_complete: bool,
    },
    /// Idle liveness signal: no records, `sequence_start == sequence_end ==`
    /// the acknowledged watermark, `previous_digest` = last accepted envelope.
    Heartbeat,
}

impl ProjectionKind {
    #[must_use]
    pub const fn is_heartbeat(&self) -> bool {
        matches!(self, Self::Heartbeat)
    }

    #[must_use]
    pub const fn as_wire(&self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Snapshot { .. } => "snapshot",
            Self::Heartbeat => "heartbeat",
        }
    }
}

/// Typed record operation. `Upsert` always carries the authoritative value;
/// `Delete` never carries one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordOperation {
    Upsert(Value),
    Delete,
}

impl RecordOperation {
    #[must_use]
    pub const fn as_wire(&self) -> &'static str {
        match self {
            Self::Upsert(_) => "upsert",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityProjectionRecord {
    pub sequence: u64,
    pub resource_type: String,
    pub resource_id: String,
    pub operation: RecordOperation,
}

#[derive(Serialize, Deserialize)]
struct RecordWire {
    sequence: u64,
    resource_type: String,
    resource_id: String,
    operation: String,
    value: Option<Value>,
}

impl Serialize for AuthorityProjectionRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (operation, value) = match &self.operation {
            RecordOperation::Upsert(value) => ("upsert", Some(value.clone())),
            RecordOperation::Delete => ("delete", None),
        };
        RecordWire {
            sequence: self.sequence,
            resource_type: self.resource_type.clone(),
            resource_id: self.resource_id.clone(),
            operation: operation.to_owned(),
            value,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AuthorityProjectionRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = RecordWire::deserialize(deserializer)?;
        let operation = match (wire.operation.as_str(), wire.value) {
            ("upsert", Some(value)) if !value.is_null() => RecordOperation::Upsert(value),
            ("delete", None) => RecordOperation::Delete,
            ("delete", Some(Value::Null)) => RecordOperation::Delete,
            (operation, _) => {
                return Err(serde::de::Error::custom(format!(
                    "projection record operation `{operation}` and value disagree"
                )));
            }
        };
        Ok(Self {
            sequence: wire.sequence,
            resource_type: wire.resource_type,
            resource_id: wire.resource_id,
            operation,
        })
    }
}

/// The signature is Ed25519 over the canonical JSON encoding of every field
/// except `signature` (object keys sorted, compact, integers only). Digests
/// are canonical `sha256:`-prefixed lowercase hex ([`Sha256Digest`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityProjectionEnvelope {
    pub schema_version: u16,
    pub installation_id: String,
    pub organization_id: String,
    pub sequence_start: u64,
    pub sequence_end: u64,
    #[serde(flatten)]
    pub kind: ProjectionKind,
    pub generated_at: String,
    pub previous_digest: Option<String>,
    pub payload_digest: String,
    pub key_id: String,
    pub records: Vec<AuthorityProjectionRecord>,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityProjectionAck {
    pub organization_id: String,
    pub highest_contiguous_sequence: u64,
    pub last_envelope_digest: String,
    pub snapshot_digest: Option<String>,
}

impl AuthorityProjectionEnvelope {
    /// Structural bounds every producer and consumer must agree on before
    /// signature verification: version, non-empty identity, canonical digests,
    /// contiguous non-zero sequences, record cardinality, and encoded size.
    #[must_use]
    pub fn within_bounds(&self) -> bool {
        let identity = self.schema_version == AUTHORITY_PROJECTION_SCHEMA_VERSION
            && !self.installation_id.trim().is_empty()
            && !self.organization_id.trim().is_empty()
            && !self.key_id.trim().is_empty()
            && Sha256Digest::is_canonical(&self.payload_digest)
            && self
                .previous_digest
                .as_deref()
                .is_none_or(Sha256Digest::is_canonical);
        let sequences = if self.kind.is_heartbeat() {
            self.records.is_empty() && self.sequence_start == self.sequence_end
        } else {
            self.sequence_start > 0
                && self.sequence_start <= self.sequence_end
                && !self.records.is_empty()
                && self.records.len() <= MAX_AUTHORITY_RECORDS_PER_BATCH
                && self.records.first().map(|record| record.sequence) == Some(self.sequence_start)
                && self.records.last().map(|record| record.sequence) == Some(self.sequence_end)
                && self
                    .records
                    .windows(2)
                    .all(|pair| pair[1].sequence == pair[0].sequence + 1)
        };
        identity
            && sequences
            && serde_json::to_vec(self)
                .is_ok_and(|encoded| encoded.len() <= MAX_AUTHORITY_ENVELOPE_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64) -> AuthorityProjectionRecord {
        AuthorityProjectionRecord {
            sequence,
            resource_type: "team".into(),
            resource_id: format!("t{sequence}"),
            operation: RecordOperation::Upsert(serde_json::json!({"policy_epoch":sequence})),
        }
    }

    fn envelope(
        kind: ProjectionKind,
        records: Vec<AuthorityProjectionRecord>,
    ) -> AuthorityProjectionEnvelope {
        let sequence_start = records.first().map_or(4, |record| record.sequence);
        let sequence_end = records.last().map_or(4, |record| record.sequence);
        AuthorityProjectionEnvelope {
            schema_version: 1,
            installation_id: "i".into(),
            organization_id: "o".into(),
            sequence_start,
            sequence_end,
            kind,
            generated_at: "2026-09-05T00:00:00Z".into(),
            previous_digest: None,
            payload_digest: format!("sha256:{}", "11".repeat(32)),
            key_id: "k".into(),
            records,
            signature: "sig".into(),
        }
    }

    #[test]
    fn projection_rejects_gaps_zero_sequences_and_unknown_versions() {
        let mut envelope = envelope(ProjectionKind::Delta, vec![record(1), record(2)]);
        assert!(envelope.within_bounds());
        envelope.records[1].sequence = 3;
        assert!(!envelope.within_bounds());
        envelope.records[1].sequence = 2;
        envelope.schema_version = 2;
        assert!(!envelope.within_bounds());

        let zero = AuthorityProjectionEnvelope {
            sequence_start: 0,
            sequence_end: 0,
            ..envelope.clone()
        };
        assert!(!zero.within_bounds());
        let mut zero_records = envelope.clone();
        zero_records.schema_version = 1;
        zero_records.records[0].sequence = 0;
        zero_records.sequence_start = 0;
        assert!(!zero_records.within_bounds());
    }

    #[test]
    fn heartbeat_carries_no_records_and_pins_the_watermark() {
        let heartbeat = envelope(ProjectionKind::Heartbeat, Vec::new());
        assert!(heartbeat.within_bounds());
        assert_eq!(heartbeat.sequence_start, heartbeat.sequence_end);
        let mut with_records = heartbeat.clone();
        with_records.records = vec![record(4)];
        assert!(!with_records.within_bounds());
        let encoded = serde_json::to_value(&heartbeat).unwrap();
        assert_eq!(encoded["kind"], "heartbeat");
        assert!(encoded.get("snapshot_id").is_none());
    }

    #[test]
    fn snapshot_kind_matches_the_flat_wire_shape() {
        let snapshot = envelope(
            ProjectionKind::Snapshot {
                snapshot_id: "snapshot-1".into(),
                snapshot_base_sequence: Some(3),
                snapshot_complete: true,
            },
            vec![record(4)],
        );
        let encoded = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(encoded["kind"], "snapshot");
        assert_eq!(encoded["snapshot_id"], "snapshot-1");
        assert_eq!(encoded["snapshot_base_sequence"], 3);
        assert_eq!(encoded["snapshot_complete"], true);
        let decoded: AuthorityProjectionEnvelope = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, snapshot);
        let delta = serde_json::to_value(envelope(ProjectionKind::Delta, vec![record(1)])).unwrap();
        assert!(delta.get("snapshot_id").is_none());
        assert!(delta.get("snapshot_complete").is_none());
    }

    #[test]
    fn record_operation_and_value_stay_coupled() {
        let upsert = serde_json::to_value(record(1)).unwrap();
        assert_eq!(upsert["operation"], "upsert");
        assert_eq!(upsert["value"]["policy_epoch"], 1);
        let delete = AuthorityProjectionRecord {
            sequence: 2,
            resource_type: "team".into(),
            resource_id: "t2".into(),
            operation: RecordOperation::Delete,
        };
        let encoded = serde_json::to_value(&delete).unwrap();
        assert_eq!(encoded["operation"], "delete");
        assert!(encoded["value"].is_null());
        assert!(encoded.as_object().unwrap().contains_key("value"));
        assert_eq!(
            serde_json::from_value::<AuthorityProjectionRecord>(encoded).unwrap(),
            delete
        );
        for malformed in [
            serde_json::json!({"sequence":1,"resource_type":"team","resource_id":"t","operation":"upsert","value":null}),
            serde_json::json!({"sequence":1,"resource_type":"team","resource_id":"t","operation":"delete","value":{"x":1}}),
            serde_json::json!({"sequence":1,"resource_type":"team","resource_id":"t","operation":"merge","value":{"x":1}}),
        ] {
            assert!(serde_json::from_value::<AuthorityProjectionRecord>(malformed).is_err());
        }
    }

    #[test]
    fn oversized_envelopes_are_out_of_bounds() {
        let mut big = record(1);
        big.operation =
            RecordOperation::Upsert(Value::String("x".repeat(MAX_AUTHORITY_ENVELOPE_BYTES)));
        assert!(!envelope(ProjectionKind::Delta, vec![big]).within_bounds());
    }
}
