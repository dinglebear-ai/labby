use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BackendId, BackendRegistry, InvariantId, identity::valid_namespace};

/// Semantic property category, used to reject unsupported backend bindings.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Nothing bad happens.
    Safety,
    /// An obligation eventually completes under declared fairness assumptions.
    Liveness,
    /// An authority or confidentiality boundary holds.
    Security,
    /// Concrete observations conform to an abstract relation.
    Refinement,
}

/// Impact of violating a catalogued property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Highest-impact property.
    Critical,
    /// High-impact property.
    High,
    /// Medium-impact property.
    Medium,
}

/// Invariant lifecycle, distinct from a scenario's reproduction status.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvariantStatus {
    /// Property currently intended to be checked.
    #[default]
    Active,
    /// Property still being specified.
    Draft,
    /// Tombstone retained permanently; the ID cannot be reused.
    Retired,
}

/// Nonempty configured handle list; semantic validation also rejects whitespace.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Handles(
    /// Adapter-owned handles, never filesystem paths interpreted by the core.
    #[schemars(length(min = 1), inner(length(min = 1)))]
    pub Vec<String>,
);

/// One named correctness property. Constructed data must pass catalog validation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    /// Permanent namespaced identifier.
    pub id: InvariantId,
    /// Human-readable property statement.
    #[schemars(length(min = 1))]
    pub title: String,
    /// Required backend semantic capability.
    pub kind: Kind,
    /// Impact of violation.
    pub severity: Severity,
    /// Project-defined model name.
    #[schemars(length(min = 1))]
    pub model: String,
    /// Optional project owner.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// Lifecycle; omitted entries are active.
    #[serde(default)]
    pub status: InvariantStatus,
    /// Optional project annotations, not interpreted by the toolkit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Backend IDs mapped to adapter-local handles. No entries means uncovered.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "deserialize_checks"
    )]
    #[schemars(with = "BTreeMap<BackendId, Handles>")]
    pub checks: BTreeMap<BackendId, Handles>,
}

fn deserialize_checks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<BackendId, Handles>, D::Error> {
    struct ChecksVisitor;
    impl<'de> serde::de::Visitor<'de> for ChecksVisitor {
        type Value = BTreeMap<BackendId, Handles>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("unique backend keys mapped to handle lists")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, handles)) = map.next_entry::<BackendId, Handles>()? {
                match result.entry(key) {
                    std::collections::btree_map::Entry::Occupied(entry) => {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate backend key: {}",
                            entry.key()
                        )));
                    }
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(handles);
                    }
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(ChecksVisitor)
}

/// Project-agnostic invariant catalog. Use `validate` before consuming bindings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Envelope version; only version 1 is supported.
    #[schemars(range(min = 1, max = 1))]
    pub schema: u32,
    /// Adopting project identity.
    #[schemars(length(min = 1))]
    pub project: String,
    /// Required prefix of every invariant ID.
    #[schemars(regex(pattern = "^[A-Z][A-Z0-9]*$"))]
    pub namespace: String,
    /// Nonempty property catalog, including retired-ID tombstones.
    #[schemars(length(min = 1))]
    pub invariant: Vec<Invariant>,
}

/// Typed parse/validation failures; callers need not classify error strings.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// Invalid JSON shape, unknown fields, or malformed IDs.
    #[error("invalid JSON catalog: {0}")]
    Json(#[from] serde_json::Error),
    /// Invalid TOML shape, duplicate keys, unknown fields, or malformed IDs.
    #[error("invalid TOML catalog: {0}")]
    Toml(#[from] toml::de::Error),
    /// Unsupported envelope version.
    #[error("unsupported catalog schema: {0}")]
    Schema(u32),
    /// A required field or collection is empty.
    #[error("empty catalog field: {0}")]
    Empty(String),
    /// Namespace does not match the stable identity syntax.
    #[error("invalid namespace: {0}")]
    Namespace(String),
    /// An invariant's namespace differs from its catalog's namespace.
    #[error("invariant {id} is outside catalog namespace {namespace}")]
    NamespaceMismatch {
        /// Rejected invariant.
        id: InvariantId,
        /// Catalog namespace.
        namespace: String,
    },
    /// Includes duplicate retired IDs: retirement does not free an ID.
    #[error("duplicate invariant: {0}")]
    DuplicateId(InvariantId),
    /// The configured adapter is not registered.
    #[error("unknown backend {backend}; registered backends: {available:?}")]
    UnknownBackend {
        /// Configured key.
        backend: BackendId,
        /// Known keys, in stable order.
        available: Vec<BackendId>,
    },
    /// Backend capability cannot discharge this property's kind.
    #[error("backend {backend} cannot check {kind:?} invariant {id}")]
    UnsupportedKind {
        /// Configured backend.
        backend: BackendId,
        /// Rejected property.
        id: InvariantId,
        /// Required capability.
        kind: Kind,
    },
    /// A handle is not known for the specified model.
    #[error("backend {backend} has no handle {handle} for model {model}")]
    UnresolvedHandle {
        /// Configured backend.
        backend: BackendId,
        /// Target model.
        model: String,
        /// Misspelled or unknown handle.
        handle: String,
    },
    /// Repeating a handle would execute the same check twice.
    #[error("duplicate handle {handle} for backend {backend}")]
    DuplicateHandle {
        /// Backend with duplicate binding.
        backend: BackendId,
        /// Repeated handle.
        handle: String,
    },
    /// A historical catalog must belong to the same project and namespace.
    #[error("catalog history belongs to a different project or namespace")]
    HistoryIdentity,
    /// A previous ID disappeared instead of retaining its tombstone.
    #[error("historical invariant removed: {0}")]
    RemovedId(InvariantId),
    /// A stable ID was reassigned or a retired ID reactivated.
    #[error("historical invariant identity reused: {0}")]
    ReusedId(InvariantId),
}

/// Immutable structurally and semantically validated catalog snapshot.
#[derive(Debug, Clone)]
pub struct ValidatedCatalog(Catalog);

impl Catalog {
    /// Parse caller-supplied JSON and validate without running any backend.
    pub fn from_json(
        input: &str,
        backends: &BackendRegistry<'_>,
    ) -> Result<ValidatedCatalog, CatalogError> {
        serde_json::from_str::<Self>(input)?.validate(backends)
    }

    /// Parse caller-supplied TOML and validate without reading files or env.
    pub fn from_toml(
        input: &str,
        backends: &BackendRegistry<'_>,
    ) -> Result<ValidatedCatalog, CatalogError> {
        toml::from_str::<Self>(input)?.validate(backends)
    }

    /// Validate every binding, including draft/retired records, against metadata.
    pub fn validate(
        self,
        backends: &BackendRegistry<'_>,
    ) -> Result<ValidatedCatalog, CatalogError> {
        if self.schema != 1 {
            return Err(CatalogError::Schema(self.schema));
        }
        nonempty(&self.project, "project")?;
        if !valid_namespace(&self.namespace) {
            return Err(CatalogError::Namespace(self.namespace));
        }
        if self.invariant.is_empty() {
            return Err(CatalogError::Empty("invariant".into()));
        }
        let mut ids = BTreeSet::new();
        for invariant in &self.invariant {
            if invariant.id.namespace() != self.namespace {
                return Err(CatalogError::NamespaceMismatch {
                    id: invariant.id.clone(),
                    namespace: self.namespace.clone(),
                });
            }
            if !ids.insert(&invariant.id) {
                return Err(CatalogError::DuplicateId(invariant.id.clone()));
            }
            nonempty(&invariant.title, "invariant.title")?;
            nonempty(&invariant.model, "invariant.model")?;
            for (key, handles) in &invariant.checks {
                let backend = backends
                    .get(key)
                    .ok_or_else(|| CatalogError::UnknownBackend {
                        backend: key.clone(),
                        available: backends.ids(),
                    })?;
                if !backend.capabilities().supports(invariant.kind) {
                    return Err(CatalogError::UnsupportedKind {
                        backend: key.clone(),
                        id: invariant.id.clone(),
                        kind: invariant.kind,
                    });
                }
                if handles.0.is_empty() {
                    return Err(CatalogError::Empty(format!("checks.{key}")));
                }
                let mut seen = BTreeSet::new();
                for handle in &handles.0 {
                    nonempty(handle, "check handle")?;
                    if !seen.insert(handle) {
                        return Err(CatalogError::DuplicateHandle {
                            backend: key.clone(),
                            handle: handle.clone(),
                        });
                    }
                    if !backend.has_handle(&invariant.model, handle) {
                        return Err(CatalogError::UnresolvedHandle {
                            backend: key.clone(),
                            model: invariant.model.clone(),
                            handle: handle.clone(),
                        });
                    }
                }
            }
        }
        Ok(ValidatedCatalog(self))
    }
}

fn nonempty(value: &str, field: &str) -> Result<(), CatalogError> {
    if value.trim().is_empty() {
        Err(CatalogError::Empty(field.into()))
    } else {
        Ok(())
    }
}

impl ValidatedCatalog {
    /// Borrow the immutable validated declaration.
    pub fn catalog(&self) -> &Catalog {
        &self.0
    }

    /// Properties with no configured checks, including their lifecycle status.
    pub fn uncovered(&self) -> impl Iterator<Item = &Invariant> {
        self.0
            .invariant
            .iter()
            .filter(|invariant| invariant.checks.is_empty())
    }

    /// Enforce ID continuity against a caller-supplied previous catalog.
    ///
    /// No history is read implicitly. Stable identity means namespace, ID,
    /// model and kind; title, owner and severity may be clarified over time.
    /// A retired ID must remain retired. Keep tombstones rather than delete IDs.
    pub fn validate_evolution(&self, previous: &Self) -> Result<(), CatalogError> {
        if self.0.project != previous.0.project || self.0.namespace != previous.0.namespace {
            return Err(CatalogError::HistoryIdentity);
        }
        let current: BTreeMap<_, _> = self.0.invariant.iter().map(|i| (&i.id, i)).collect();
        for old in &previous.0.invariant {
            let new = current
                .get(&old.id)
                .ok_or_else(|| CatalogError::RemovedId(old.id.clone()))?;
            if new.kind != old.kind
                || new.model != old.model
                || (old.status == InvariantStatus::Retired
                    && new.status != InvariantStatus::Retired)
            {
                return Err(CatalogError::ReusedId(old.id.clone()));
            }
        }
        Ok(())
    }
}

/// Generate the public schema from the same types the core deserializes.
/// Cross-field namespace, registry and history constraints remain runtime rules.
pub fn catalog_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(Catalog);
    schema.insert(
        "$id".into(),
        "https://dinglebear.ai/schemas/verify/invariants/1".into(),
    );
    schema
}
