//! Canonical `sha256:<64 lowercase hex>` digest vocabulary.
//!
//! Every Labby crate that carries a content digest on a wire or in durable
//! state uses this one representation instead of re-implementing the check.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A validation failure for a digest literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestError {
    /// The literal did not start with `sha256:`.
    MissingPrefix,
    /// The hex body was not exactly 64 lowercase hexadecimal characters.
    InvalidHex,
}

impl fmt::Display for DigestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingPrefix => "digest must start with `sha256:`",
            Self::InvalidHex => "digest body must be 64 lowercase hexadecimal characters",
        })
    }
}

impl std::error::Error for DigestError {}

/// A canonical SHA-256 digest literal, `sha256:` followed by 64 lowercase hex digits.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// The mandatory algorithm prefix.
    pub const PREFIX: &'static str = "sha256:";
    /// Length of the hex body.
    pub const HEX_LEN: usize = 64;
    /// Length of the full canonical literal.
    pub const ENCODED_LEN: usize = Self::PREFIX.len() + Self::HEX_LEN;

    /// Parse a canonical literal. Uppercase hex and missing prefixes are rejected.
    pub fn parse(value: impl Into<String>) -> Result<Self, DigestError> {
        let value = value.into();
        Self::validate(&value)?;
        Ok(Self(value))
    }

    /// Validate a literal without allocating.
    pub fn validate(value: &str) -> Result<(), DigestError> {
        let Some(hex) = value.strip_prefix(Self::PREFIX) else {
            return Err(DigestError::MissingPrefix);
        };
        if hex.len() != Self::HEX_LEN
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DigestError::InvalidHex);
        }
        Ok(())
    }

    /// `true` when the literal is canonical.
    #[must_use]
    pub fn is_canonical(value: &str) -> bool {
        Self::validate(value).is_ok()
    }

    /// SHA-256 of arbitrary bytes as the canonical literal.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(bytes);
        let mut raw = [0_u8; 32];
        raw.copy_from_slice(&digest);
        Self::from_bytes(&raw)
    }

    /// Encode raw digest bytes as the canonical literal.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut encoded = String::with_capacity(Self::ENCODED_LEN);
        encoded.push_str(Self::PREFIX);
        for byte in bytes {
            encoded.push(HEX[usize::from(byte >> 4)]);
            encoded.push(HEX[usize::from(byte & 0x0f)]);
        }
        Self(encoded)
    }

    /// The full `sha256:...` literal.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-character hex body without the prefix.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.0[Self::PREFIX.len()..]
    }
}

const HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

impl TryFrom<String> for Sha256Digest {
    type Error = DigestError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        value.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_literal_round_trips_and_variants_are_rejected() {
        let literal = format!("sha256:{}", "a".repeat(64));
        let digest = Sha256Digest::parse(literal.clone()).unwrap();
        assert_eq!(digest.as_str(), literal);
        assert_eq!(digest.hex(), "a".repeat(64));
        assert_eq!(
            Sha256Digest::parse(format!("sha256:{}", "A".repeat(64))).unwrap_err(),
            DigestError::InvalidHex
        );
        assert_eq!(
            Sha256Digest::parse("a".repeat(64)).unwrap_err(),
            DigestError::MissingPrefix
        );
        assert_eq!(
            Sha256Digest::parse(format!("sha256:{}", "a".repeat(63))).unwrap_err(),
            DigestError::InvalidHex
        );
        assert_eq!(
            Sha256Digest::from_bytes(&[0xab; 32]).as_str(),
            format!("sha256:{}", "ab".repeat(32))
        );
        assert_eq!(
            Sha256Digest::of(b"").hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let encoded = serde_json::to_string(&digest).unwrap();
        assert_eq!(encoded, format!("\"{literal}\""));
        assert!(serde_json::from_str::<Sha256Digest>("\"sha256:short\"").is_err());
    }
}
