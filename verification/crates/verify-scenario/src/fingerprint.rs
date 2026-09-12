//! Content fingerprinting for corpus deduplication.
//!
//! Two counterexamples that differ only in irrelevant detail must land on the
//! same fingerprint, or the corpus rots into thousands of near-duplicates. That
//! work is normalization's; this module only hashes what it is given, over a
//! canonical byte form so the same scenario hashes identically on every machine
//! and every serde version.
//!
//! SHA-256 with an `s256:` prefix. The prefix exists so a future change of hash
//! is a visible migration rather than a silent mismatch.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::envelope::Scenario;

/// Prefix marking the hash algorithm used.
pub const FINGERPRINT_PREFIX: &str = "s256:";

/// Fingerprint the parts of a scenario that determine its behavior.
///
/// Deliberately excludes `origin`, `bounds`, `status`, and any existing
/// `fingerprint`: provenance and reproduction status are metadata about the
/// trace, not the trace. Two identical traces found by different backends are
/// the same scenario and must collide.
pub fn fingerprint(scenario: &Scenario) -> String {
    let mut canonical = Map::new();
    canonical.insert(
        "project".to_owned(),
        Value::String(scenario.project.clone()),
    );
    canonical.insert("model".to_owned(), Value::String(scenario.model.clone()));
    canonical.insert(
        "invariant".to_owned(),
        Value::String(scenario.invariant.as_str().to_owned()),
    );
    canonical.insert("initial".to_owned(), scenario.initial_value());
    canonical.insert("steps".to_owned(), Value::Array(scenario.steps.clone()));
    canonical.insert(
        "expect".to_owned(),
        serde_json::to_value(scenario.expect).unwrap_or(Value::Null),
    );

    let bytes = canonical_bytes(&Value::Object(canonical));
    let digest = Sha256::digest(&bytes);
    let mut out = String::with_capacity(FINGERPRINT_PREFIX.len() + digest.len() * 2);
    out.push_str(FINGERPRINT_PREFIX);
    for byte in digest {
        use std::fmt::Write as _;
        // sha2 0.11 returns a hybrid_array::Array, which does not implement
        // LowerHex; hex-encode explicitly rather than depending on a hex crate.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Serialize to a canonical byte form: object keys sorted, no insignificant
/// whitespace. `serde_json::Map` is a `BTreeMap` unless the `preserve_order`
/// feature is on, so this walks the value explicitly rather than trusting a
/// feature flag a downstream crate could turn on and silently change hashes.
fn canonical_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push(b'{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(
                    serde_json::to_string(key)
                        .unwrap_or_else(|_| "\"\"".to_owned())
                        .as_bytes(),
                );
                out.push(b':');
                if let Some(nested) = map.get(*key) {
                    write_canonical(nested, out);
                }
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(
            serde_json::to_string(other)
                .unwrap_or_else(|_| "null".to_owned())
                .as_bytes(),
        ),
    }
}
