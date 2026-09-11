//! Shared first-party Skill generation admission limits.

use labby_runtime::skills::limits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmissionLimits {
    pub(crate) active_skills: usize,
    pub(crate) aggregate_bytes: usize,
    pub(crate) per_skill_bytes: usize,
    pub(crate) total_resources: usize,
    pub(crate) live_candidate_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmissionTotals {
    pub(crate) skills: usize,
    pub(crate) bytes: usize,
    pub(crate) max_skill_bytes: usize,
    pub(crate) resources: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmissionViolation {
    pub(crate) kind: &'static str,
    pub(crate) actual: usize,
    pub(crate) limit: usize,
}

impl AdmissionLimits {
    pub(crate) fn first_violation(self, totals: AdmissionTotals) -> Option<AdmissionViolation> {
        [
            ("active_skills", totals.skills, self.active_skills),
            ("aggregate_bytes", totals.bytes, self.aggregate_bytes),
            (
                "per_skill_bytes",
                totals.max_skill_bytes,
                self.per_skill_bytes,
            ),
            ("total_resources", totals.resources, self.total_resources),
            (
                "live_candidate_bytes",
                totals.bytes,
                self.live_candidate_bytes,
            ),
        ]
        .into_iter()
        .find_map(|(kind, actual, limit)| {
            (actual > limit).then_some(AdmissionViolation {
                kind,
                actual,
                limit,
            })
        })
    }
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            active_skills: limits::MAX_SKILLS_PER_UPSTREAM,
            aggregate_bytes: 64 * 1024 * 1024,
            per_skill_bytes: 16 * 1024 * 1024,
            total_resources: limits::MAX_SKILLS_PER_UPSTREAM * limits::MAX_RESOURCES_PER_SKILL,
            live_candidate_bytes: 64 * 1024 * 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_generation_admission_limits_share_one_ordered_predicate() {
        let limits = AdmissionLimits {
            active_skills: 1,
            aggregate_bytes: 10,
            per_skill_bytes: 6,
            total_resources: 2,
            live_candidate_bytes: 8,
        };
        assert_eq!(
            limits.first_violation(AdmissionTotals {
                skills: 2,
                bytes: 20,
                max_skill_bytes: 7,
                resources: 3,
            }),
            Some(AdmissionViolation {
                kind: "active_skills",
                actual: 2,
                limit: 1,
            })
        );
        assert!(
            limits
                .first_violation(AdmissionTotals {
                    skills: 1,
                    bytes: 8,
                    max_skill_bytes: 6,
                    resources: 2,
                })
                .is_none()
        );
    }
}
