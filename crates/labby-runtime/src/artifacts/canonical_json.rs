//! Cross-runtime canonical JSON used for Artifact content digests.
//!
//! Depot's frozen v1 contract sorts object keys lexicographically, preserves
//! array order, and uses ordinary JSON scalar encoding. Rust must produce the
//! same bytes so revision identifiers remain portable across products.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::ArtifactError;

/// Encode any serializable value as deterministic canonical JSON bytes.
pub fn to_canonical_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, ArtifactError> {
    let value = serde_json::to_value(value)?;
    let mut out = Vec::new();
    write_value(&value, &mut out)?;
    Ok(out)
}

/// Return a lowercase `sha256:<hex>` digest of raw bytes.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let digest_bytes: &[u8] = digest.as_ref();
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for &byte in digest_bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

/// Return a lowercase SHA-256 digest of canonical JSON.
pub fn digest<T: Serialize + ?Sized>(value: &T) -> Result<String, ArtifactError> {
    Ok(sha256_bytes(&to_canonical_vec(value)?))
}

fn write_value(value: &Value, out: &mut Vec<u8>) -> Result<(), ArtifactError> {
    match value {
        Value::Object(map) => {
            out.push(b'{');
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key)?;
                out.push(b':');
                write_value(value, out)?;
            }
            out.push(b'}');
        }
        Value::Array(values) => {
            out.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_value(value, out)?;
            }
            out.push(b']');
        }
        scalar => serde_json::to_writer(out, scalar)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    #[test]
    fn sorts_object_keys_and_preserves_array_order() {
        let value = json!({"z": 1, "a": [3, 2, 1], "m": {"y": true, "b": null}});
        assert_eq!(
            String::from_utf8(to_canonical_vec(&value).expect("canonical JSON")).expect("UTF-8"),
            r#"{"a":[3,2,1],"m":{"b":null,"y":true},"z":1}"#
        );
    }

    #[test]
    fn map_implementation_does_not_affect_digest() {
        let mut left = BTreeMap::new();
        left.insert("b", 2_u8);
        left.insert("a", 1_u8);
        assert_eq!(
            digest(&left).unwrap(),
            digest(&json!({"a": 1, "b": 2})).unwrap()
        );
    }
}
