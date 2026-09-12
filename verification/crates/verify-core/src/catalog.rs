//! The invariant catalog and its validation rules.
//!
//! Parsing a catalog is the easy half. The rules below are the reason this
//! crate exists: each one closes a way for a catalog to look fine and report
//! green while checking nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::backend::{BackendId, Capabilities};
use crate::invariant::{InvariantId, Kind, Severity, Status};

/// Schema version of the catalog format.
pub const CATALOG_SCHEMA: u32 = 1;

/// A parsed `invariants.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub project: String,
    /// Identifier prefix every invariant id in this catalog must carry.
    pub namespace: String,
    #[serde(default, rename = "invariant")]
    pub invariants: Vec<Invariant>,
}

/// One catalogued property.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    pub id: InvariantId,
    pub title: String,
    pub kind: Kind,
    pub severity: Severity,
    /// Target model name, resolved against the project's target registry.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Backend id to the handles that backend resolves for this invariant.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<BackendId, Vec<String>>,
}

impl Invariant {
    /// Whether any backend claims this invariant.
    pub fn is_covered(&self) -> bool {
        self.checks.values().any(|handles| !handles.is_empty())
    }
}

/// One reason a catalog was rejected.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CatalogError {
    #[error("catalog declares schema {found}, but this tool understands {CATALOG_SCHEMA}")]
    Schema { found: u32 },

    #[error(
        "catalog namespace `{namespace}` must be uppercase alphanumeric starting with a letter"
    )]
    Namespace { namespace: String },

    #[error("invariant id `{id}` appears more than once")]
    DuplicateId { id: InvariantId },

    #[error(
        "invariant id `{id}` does not start with the catalog namespace `{namespace}`; \
         ids from another project would otherwise validate cleanly here"
    )]
    NamespaceMismatch { id: InvariantId, namespace: String },

    #[error(
        "invariant `{id}` binds backend `{backend}`, which is not registered; known backends: {known}"
    )]
    UnknownBackend {
        id: InvariantId,
        backend: BackendId,
        known: String,
    },

    #[error(
        "invariant `{id}` is a {kind} property but backend `{backend}` does not claim that kind; \
         binding it would report green without establishing anything"
    )]
    CapabilityMismatch {
        id: InvariantId,
        kind: Kind,
        backend: BackendId,
    },

    #[error("invariant `{id}` binds backend `{backend}` with no handles")]
    EmptyHandles { id: InvariantId, backend: BackendId },
}

impl Catalog {
    /// Parse a catalog from TOML text.
    pub fn parse(text: &str) -> Result<Self, CatalogParseError> {
        toml::from_str(text).map_err(|source| CatalogParseError {
            message: source.to_string(),
        })
    }

    /// Check every structural rule, returning *all* violations rather than the
    /// first: a catalog author fixing one id at a time is a slow loop.
    ///
    /// `backends` maps each registered backend to what it can honestly claim.
    /// Pass an empty map only when no backends are registered yet — every
    /// binding will then be reported as unknown, which is the correct answer.
    pub fn validate(
        &self,
        backends: &BTreeMap<BackendId, Capabilities>,
    ) -> Result<(), Vec<CatalogError>> {
        let mut errors = self.validate_structure().err().unwrap_or_default();
        for invariant in &self.invariants {
            for (backend, handles) in &invariant.checks {
                let Some(capabilities) = backends.get(backend) else {
                    errors.push(CatalogError::UnknownBackend {
                        id: invariant.id.clone(),
                        backend: backend.clone(),
                        known: render_known(backends),
                    });
                    continue;
                };
                if handles.is_empty() {
                    errors.push(CatalogError::EmptyHandles {
                        id: invariant.id.clone(),
                        backend: backend.clone(),
                    });
                }
                if !capabilities.claims(invariant.kind) {
                    errors.push(CatalogError::CapabilityMismatch {
                        id: invariant.id.clone(),
                        kind: invariant.kind,
                        backend: backend.clone(),
                    });
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate schema and invariant identity without resolving backend bindings.
    /// Replay uses this subset because it executes targets, not search backends.
    pub fn validate_structure(&self) -> Result<(), Vec<CatalogError>> {
        let mut errors = Vec::new();

        if self.schema != CATALOG_SCHEMA {
            errors.push(CatalogError::Schema { found: self.schema });
        }
        if !is_valid_namespace(&self.namespace) {
            errors.push(CatalogError::Namespace {
                namespace: self.namespace.clone(),
            });
        }

        let mut seen: BTreeSet<&InvariantId> = BTreeSet::new();
        for invariant in &self.invariants {
            if !seen.insert(&invariant.id) {
                errors.push(CatalogError::DuplicateId {
                    id: invariant.id.clone(),
                });
            }
            if invariant.id.namespace() != self.namespace {
                errors.push(CatalogError::NamespaceMismatch {
                    id: invariant.id.clone(),
                    namespace: self.namespace.clone(),
                });
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Ids no backend claims. A legitimate state to ship with, as long as the
    /// report names them instead of printing a count.
    pub fn uncovered(&self) -> Vec<&InvariantId> {
        self.invariants
            .iter()
            .filter(|invariant| invariant.status != Status::Retired && !invariant.is_covered())
            .map(|invariant| &invariant.id)
            .collect()
    }
}

fn render_known(backends: &BTreeMap<BackendId, Capabilities>) -> String {
    if backends.is_empty() {
        return "none registered".to_owned();
    }
    backends
        .keys()
        .map(BackendId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_valid_namespace(namespace: &str) -> bool {
    let mut bytes = namespace.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_uppercase() && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// A catalog that could not be parsed at all.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("catalog is not valid TOML for this schema: {message}")]
pub struct CatalogParseError {
    pub message: String,
}

#[cfg(test)]
mod tests;
