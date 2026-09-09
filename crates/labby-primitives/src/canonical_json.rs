//! Canonical JSON profile shared by every signed or fingerprinted Labby wire
//! payload (authority projection envelopes, Depot operation schema
//! fingerprints, delegated assertion bodies).
//!
//! The profile is deliberately narrow so two independent implementations
//! (Labby in Rust, Depot in Elixir) produce byte-identical output:
//!
//! - object keys are sorted lexicographically by their UTF-8 bytes;
//! - no insignificant whitespace;
//! - strings use `serde_json`'s escaping (control characters, `"` and `\`
//!   escaped; non-ASCII emitted verbatim as UTF-8);
//! - numbers must be integers. A float never reaches a signer or a
//!   fingerprint: encoding fails closed instead of guessing a textual form.

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::digest::Sha256Digest;

/// Why a value has no canonical encoding.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CanonicalJsonError {
    /// A non-integer number was present. Canonical payloads carry integers only.
    #[error("canonical JSON rejects non-integer number at `{path}`")]
    NonIntegerNumber { path: String },
    /// The input could not be converted to a JSON value or written out.
    #[error("canonical JSON encoding failed: {0}")]
    Encode(String),
}

/// Encode any serializable value under the canonical profile.
pub fn to_canonical_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
    let value = serde_json::to_value(value)
        .map_err(|error| CanonicalJsonError::Encode(error.to_string()))?;
    canonical_value(&value)
}

/// Encode an already-parsed JSON value under the canonical profile.
pub fn canonical_value(value: &Value) -> Result<Vec<u8>, CanonicalJsonError> {
    let mut out = Vec::new();
    let mut path = String::from("$");
    write_value(value, &mut out, &mut path)?;
    Ok(out)
}

/// Lowercase SHA-256 hex (no algorithm prefix) of the canonical encoding.
/// This is the Depot operation `schemaFingerprint` form.
pub fn fingerprint_hex<T: Serialize + ?Sized>(value: &T) -> Result<String, CanonicalJsonError> {
    Ok(Sha256Digest::of(&to_canonical_vec(value)?).hex().to_owned())
}

/// `sha256:`-prefixed digest of the canonical encoding.
pub fn digest<T: Serialize + ?Sized>(value: &T) -> Result<Sha256Digest, CanonicalJsonError> {
    Ok(Sha256Digest::of(&to_canonical_vec(value)?))
}

fn write_value(
    value: &Value,
    out: &mut Vec<u8>,
    path: &mut String,
) -> Result<(), CanonicalJsonError> {
    match value {
        Value::Object(map) => {
            out.push(b'{');
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            for (index, (key, child)) in entries.into_iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key)
                    .map_err(|error| CanonicalJsonError::Encode(error.to_string()))?;
                out.push(b':');
                let mark = path.len();
                path.push('.');
                path.push_str(key);
                write_value(child, out, path)?;
                path.truncate(mark);
            }
            out.push(b'}');
        }
        Value::Array(values) => {
            out.push(b'[');
            for (index, child) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                let mark = path.len();
                path.push_str(&format!("[{index}]"));
                write_value(child, out, path)?;
                path.truncate(mark);
            }
            out.push(b']');
        }
        Value::Number(number) => {
            if number.as_i64().is_none() && number.as_u64().is_none() {
                return Err(CanonicalJsonError::NonIntegerNumber { path: path.clone() });
            }
            serde_json::to_writer(&mut *out, number)
                .map_err(|error| CanonicalJsonError::Encode(error.to_string()))?;
        }
        scalar => serde_json::to_writer(&mut *out, scalar)
            .map_err(|error| CanonicalJsonError::Encode(error.to_string()))?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_sort_bytewise_and_whitespace_is_dropped() {
        let value = json!({"z": 1, "a": [3, {"y": true, "b": null}], "é": "ü", "B": "x"});
        assert_eq!(
            String::from_utf8(canonical_value(&value).unwrap()).unwrap(),
            r#"{"B":"x","a":[3,{"b":null,"y":true}],"z":1,"é":"ü"}"#
        );
    }

    #[test]
    fn matches_the_depot_golden_envelope_encoding() {
        let value = json!({"sequence_start":1,"records":[],"previous_digest":null,"payload_digest":"sha256:placeholder","organization_id":"org-1","kind":"delta","key_id":"key-1","installation_id":"install-1","generated_at":"2026-09-05T00:00:00Z","sequence_end":1,"schema_version":1});
        assert_eq!(
            String::from_utf8(canonical_value(&value).unwrap()).unwrap(),
            "{\"generated_at\":\"2026-09-05T00:00:00Z\",\"installation_id\":\"install-1\",\"key_id\":\"key-1\",\"kind\":\"delta\",\"organization_id\":\"org-1\",\"payload_digest\":\"sha256:placeholder\",\"previous_digest\":null,\"records\":[],\"schema_version\":1,\"sequence_end\":1,\"sequence_start\":1}"
        );
    }

    #[test]
    fn floats_never_reach_a_signer() {
        let error = canonical_value(&json!({"a": {"b": [1, 2.5]}})).unwrap_err();
        assert_eq!(
            error,
            CanonicalJsonError::NonIntegerNumber {
                path: "$.a.b[1]".into()
            }
        );
        assert!(to_canonical_vec(&1.0_f64).is_err());
        assert!(to_canonical_vec(&u64::MAX).is_ok());
        assert!(to_canonical_vec(&i64::MIN).is_ok());
    }

    #[test]
    fn empty_records_digest_is_the_heartbeat_payload_digest() {
        assert_eq!(
            digest(&json!([])).unwrap().hex(),
            "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
        );
        assert_eq!(fingerprint_hex(&json!({})).unwrap().len(), 64);
    }
}
