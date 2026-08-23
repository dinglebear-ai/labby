//! Provider-neutral Skill compatibility and availability vocabulary.
//!
//! This module describes whether Labby can safely offer a discovered Skill. It
//! deliberately does not represent execution authorization: discovery,
//! compatibility, and availability never grant tools or side-effect access.

use serde::{Deserialize, Deserializer, Serialize};

/// Labby's disposition for one compatibility feature or requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillCompatibilityClassification {
    /// Labby understands and supports the feature directly.
    Supported,
    /// The source value is retained for consumers but has no runtime effect.
    PreservedHint,
    /// A provider or surface adapter can safely translate the feature.
    Adaptable,
    /// A required runtime, tool, or other dependency is not currently present.
    DependencyUnavailable,
    /// The value is malformed or contradicts the applicable contract.
    Invalid,
    /// Policy permits preserving the metadata but forbids offering the Skill.
    PolicyBlocked,
}

impl SkillCompatibilityClassification {
    /// Whether this disposition prevents Labby from safely offering the Skill.
    #[must_use]
    pub const fn blocks_availability(self) -> bool {
        matches!(
            self,
            Self::DependencyUnavailable | Self::Invalid | Self::PolicyBlocked
        )
    }
}

/// Classification of one named source feature or requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCompatibilityItem {
    /// Stable provider-neutral feature or requirement name.
    pub name: String,
    pub classification: SkillCompatibilityClassification,
    /// Operator-readable context. It must not contain secrets or package bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SkillCompatibilityItem {
    #[must_use]
    pub fn new(name: impl Into<String>, classification: SkillCompatibilityClassification) -> Self {
        Self {
            name: name.into(),
            classification,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Compact explanation of whether Labby can safely offer a Skill.
///
/// `available` is derived exclusively from compatibility classifications. It
/// means the Skill may be offered for discovery or activation; it never grants
/// execution authorization, tools, filesystem, network, shell, or secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillAvailabilitySummary {
    available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    items: Vec<SkillCompatibilityItem>,
}

impl SkillAvailabilitySummary {
    /// A validated Skill with no additional compatibility requirements.
    #[must_use]
    pub const fn available() -> Self {
        Self {
            available: true,
            items: Vec::new(),
        }
    }

    /// Build a summary and fail closed when any item blocks availability.
    #[must_use]
    pub fn from_items(items: impl IntoIterator<Item = SkillCompatibilityItem>) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();
        let available = !items
            .iter()
            .any(|item| item.classification.blocks_availability());
        Self { available, items }
    }

    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.available
    }

    #[must_use]
    pub fn items(&self) -> &[SkillCompatibilityItem] {
        &self.items
    }
}

impl<'de> Deserialize<'de> for SkillAvailabilitySummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            available: bool,
            #[serde(default)]
            items: Vec<SkillCompatibilityItem>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let summary = Self::from_items(wire.items);
        if summary.available != wire.available {
            return Err(serde::de::Error::custom(
                "available contradicts compatibility items",
            ));
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_blocking_classifications_remain_available() {
        let summary = SkillAvailabilitySummary::from_items([
            SkillCompatibilityItem::new(
                "frontmatter.description",
                SkillCompatibilityClassification::Supported,
            ),
            SkillCompatibilityItem::new(
                "claude.allowed-tools",
                SkillCompatibilityClassification::PreservedHint,
            ),
            SkillCompatibilityItem::new(
                "provider.command",
                SkillCompatibilityClassification::Adaptable,
            ),
        ]);

        assert!(summary.is_available());
    }

    #[test]
    fn every_blocking_classification_fails_availability_closed() {
        for classification in [
            SkillCompatibilityClassification::DependencyUnavailable,
            SkillCompatibilityClassification::Invalid,
            SkillCompatibilityClassification::PolicyBlocked,
        ] {
            let summary = SkillAvailabilitySummary::from_items([SkillCompatibilityItem::new(
                "requirement",
                classification,
            )
            .with_detail("operator-visible reason")]);

            assert!(!summary.is_available(), "{classification:?} must block");
        }
    }

    #[test]
    fn serde_is_stable_and_contains_no_authorization_grant() {
        let summary = SkillAvailabilitySummary::from_items([SkillCompatibilityItem::new(
            "claude.allowed-tools",
            SkillCompatibilityClassification::PreservedHint,
        )]);

        let value = serde_json::to_value(summary).expect("availability JSON");
        assert_eq!(
            value,
            json!({
                "available": true,
                "items": [{
                    "name": "claude.allowed-tools",
                    "classification": "preserved_hint"
                }]
            })
        );
        assert!(value.get("authorized").is_none());
        assert!(value.get("allowed_tools").is_none());
    }

    #[test]
    fn serde_rejects_contradictory_availability() {
        let value =
            json!({"available": true, "items": [{"name": "x", "classification": "invalid"}]});
        assert!(serde_json::from_value::<SkillAvailabilitySummary>(value).is_err());
    }
}
