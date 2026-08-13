//! Resource digest parsing and verification for the skills extension (SEP-2640).
//!
//! # What a digest match does and does not prove
//!
//! SEP-2640 is explicit that this is a *consistency* check, not a trust
//! boundary: digests are unsigned and are supplied by the same server that
//! supplies the content, and the spec names gateways specifically —
//! "[a]ny intermediary on the path, such as a gateway, can rewrite both the
//! listing and the content together. Hosts MUST NOT treat a digest match as a
//! security boundary."
//!
//! Labby is precisely such an intermediary, so nothing here should be described
//! as tamper detection. What verification does buy is that a file Labby relays
//! is the file the entry it also relayed promised — which catches upstream bugs,
//! truncation, and staleness after a skill is updated.

use sha2::{Digest, Sha256};

use crate::error::ToolError;

/// The only digest algorithm this implementation accepts.
pub const DIGEST_ALGORITHM: &str = "sha256";

const DIGEST_PREFIX: &str = "sha256:";
const SHA256_HEX_LEN: usize = 64;

/// A parsed `sha256:<64 lowercase hex>` digest from a skill entry's `resources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDigest {
    hex: String,
}

impl ResourceDigest {
    /// The lowercase hex body, without the algorithm prefix.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// Canonical `sha256:<hex>` wire form.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{DIGEST_PREFIX}{}", self.hex)
    }

    /// Compute the digest of `bytes`.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self {
            hex: hex_lower(&hasher.finalize()),
        }
    }

    /// True when `bytes` hash to this digest.
    #[must_use]
    pub fn matches(&self, bytes: &[u8]) -> bool {
        Self::of_bytes(bytes) == *self
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        // Writing to a String is infallible.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Parse a digest string from a skill entry.
///
/// Rejects any algorithm other than `sha256`, any length other than 64, and
/// uppercase hex. Uppercase is rejected rather than normalized so that a digest
/// has exactly one representation and cannot compare unequal to itself across
/// two code paths that normalize differently.
pub fn parse_digest(raw: &str) -> Result<ResourceDigest, ToolError> {
    let Some(hex) = raw.strip_prefix(DIGEST_PREFIX) else {
        let algorithm = raw.split_once(':').map_or(raw, |(alg, _)| alg);
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!(
                "unsupported skill resource digest algorithm `{algorithm}`: only `{DIGEST_ALGORITHM}` is accepted"
            ),
        });
    };
    if hex.len() != SHA256_HEX_LEN {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!(
                "skill resource digest must carry exactly {SHA256_HEX_LEN} hex characters, found {}",
                hex.len()
            ),
        });
    }
    if !hex.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() && c.is_ascii_hexdigit()) {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "skill resource digest must be lowercase hexadecimal".to_string(),
        });
    }
    Ok(ResourceDigest {
        hex: hex.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn parses_valid_digest() {
        let digest = parse_digest(&format!("sha256:{EMPTY_SHA256}")).expect("valid");
        assert_eq!(digest.hex(), EMPTY_SHA256);
        assert_eq!(digest.to_wire(), format!("sha256:{EMPTY_SHA256}"));
    }

    #[test]
    fn computes_and_matches_known_vector() {
        let digest = ResourceDigest::of_bytes(b"");
        assert_eq!(digest.hex(), EMPTY_SHA256);
        assert!(digest.matches(b""));
        assert!(!digest.matches(b"x"));
    }

    #[test]
    fn rejects_wrong_algorithm() {
        for raw in [
            "sha1:0000000000000000000000000000000000000000",
            "md5:d41d8cd98f00b204e9800998ecf8427e",
            "blake3:abc",
            EMPTY_SHA256,
        ] {
            assert!(parse_digest(raw).is_err(), "should reject {raw}");
        }
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(parse_digest("sha256:abc").is_err());
        assert!(parse_digest(&format!("sha256:{EMPTY_SHA256}00")).is_err());
    }

    #[test]
    fn rejects_uppercase_hex() {
        assert!(parse_digest(&format!("sha256:{}", EMPTY_SHA256.to_uppercase())).is_err());
    }

    #[test]
    fn rejects_non_hex_body() {
        let not_hex = "z".repeat(64);
        assert!(parse_digest(&format!("sha256:{not_hex}")).is_err());
    }

    #[test]
    fn mismatch_is_detected() {
        let digest = parse_digest(&format!("sha256:{EMPTY_SHA256}")).expect("valid");
        assert!(digest.matches(b""));
        assert!(!digest.matches(b"tampered"));
    }
}
