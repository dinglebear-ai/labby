//! Explainable exposure decisions for validated Agent Skills.
//!
//! Enforcement and operator explanation must evaluate the same compiled policy.
//! Keeping the decision here prevents the operator surface from reverse-engineering
//! a reason from a boolean after the policy context has been discarded.

use super::super::types::SkillExposurePolicy;

/// Why a validated skill is exposed or hidden by the current upstream policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillExposureReason {
    AllowAll,
    MatchedPattern,
    NotMatched,
}

impl SkillExposureReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AllowAll => "allow_all",
            Self::MatchedPattern => "matched_pattern",
            Self::NotMatched => "not_matched",
        }
    }
}

/// The complete v1 exposure result for one validated skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillExposureDecision {
    pub(crate) exposed: bool,
    pub(crate) reason: SkillExposureReason,
    pub(crate) matched_pattern: Option<String>,
}

impl SkillExposureDecision {
    pub(crate) fn evaluate(policy: &SkillExposurePolicy, skill_name: &str) -> Self {
        if policy.is_unrestricted() {
            Self {
                exposed: true,
                reason: SkillExposureReason::AllowAll,
                matched_pattern: Some("*".to_string()),
            }
        } else if let Some(pattern) = policy.matched_by(skill_name) {
            Self {
                exposed: true,
                reason: SkillExposureReason::MatchedPattern,
                matched_pattern: Some(pattern),
            }
        } else {
            Self {
                exposed: false,
                reason: SkillExposureReason::NotMatched,
                matched_pattern: None,
            }
        }
    }

    pub(crate) const fn status(&self) -> &'static str {
        if self.exposed { "exposed" } else { "hidden" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_policy_is_explained_as_allow_all() {
        let decision = SkillExposureDecision::evaluate(&SkillExposurePolicy::all(), "review-pr");

        assert!(decision.exposed);
        assert_eq!(decision.reason, SkillExposureReason::AllowAll);
        assert_eq!(decision.matched_pattern.as_deref(), Some("*"));
    }

    #[test]
    fn allowlist_reports_the_matching_pattern() {
        let policy = SkillExposurePolicy::from_patterns(vec!["review-*".into()]).expect("policy");
        let decision = SkillExposureDecision::evaluate(&policy, "review-pr");

        assert!(decision.exposed);
        assert_eq!(decision.reason, SkillExposureReason::MatchedPattern);
        assert_eq!(decision.matched_pattern.as_deref(), Some("review-*"));
    }

    #[test]
    fn allowlist_reports_a_non_match_without_inventing_a_rule() {
        let policy = SkillExposurePolicy::from_patterns(vec!["review-*".into()]).expect("policy");
        let decision = SkillExposureDecision::evaluate(&policy, "deploy");

        assert!(!decision.exposed);
        assert_eq!(decision.reason, SkillExposureReason::NotMatched);
        assert_eq!(decision.matched_pattern, None);
    }
}
