use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A malformed stable identifier.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {kind} identifier: {value}")]
pub struct IdentityError {
    /// Identifier category, not project-specific vocabulary.
    pub kind: &'static str,
    /// Rejected identifier.
    pub value: String,
}

pub(crate) fn valid_namespace(value: &str) -> bool {
    value.starts_with(|c: char| c.is_ascii_uppercase())
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Permanent namespaced identity, such as `PROJECT-REQ-001`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InvariantId(String);

impl InvariantId {
    /// Borrow the canonical identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Borrow the namespace before the first hyphen.
    pub fn namespace(&self) -> &str {
        self.0.split('-').next().unwrap_or_default()
    }
}

impl TryFrom<String> for InvariantId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parts: Vec<_> = value.split('-').collect();
        if parts.len() == 3
            && valid_namespace(parts[0])
            && !parts[1].is_empty()
            && parts[1].bytes().all(|b| b.is_ascii_uppercase())
            && parts[2].len() >= 3
            && parts[2].bytes().all(|b| b.is_ascii_digit())
        {
            Ok(Self(value))
        } else {
            Err(IdentityError {
                kind: "invariant",
                value,
            })
        }
    }
}

impl From<InvariantId> for String {
    fn from(value: InvariantId) -> Self {
        value.0
    }
}

impl fmt::Display for InvariantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl JsonSchema for InvariantId {
    fn schema_name() -> Cow<'static, str> {
        "InvariantId".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[A-Z][A-Z0-9]*-[A-Z]+-[0-9]{3,}$"})
    }
}

/// Extensible backend key. This type intentionally does not enumerate engines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BackendId(String);

impl BackendId {
    /// Borrow the backend key.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BackendId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.starts_with(|c: char| c.is_ascii_lowercase())
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            Ok(Self(value))
        } else {
            Err(IdentityError {
                kind: "backend",
                value,
            })
        }
    }
}

impl From<BackendId> for String {
    fn from(value: BackendId) -> Self {
        value.0
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl JsonSchema for BackendId {
    fn schema_name() -> Cow<'static, str> {
        "BackendId".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type": "string", "pattern": "^[a-z][a-z0-9_]*$"})
    }
}
