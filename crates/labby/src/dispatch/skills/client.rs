//! Runtime context construction for the native Agent Skills protocol.
//!
//! Context-free dispatch is intentionally first-party-only. Surface adapters
//! that hold a live GatewayManager must call the explicit scoped dispatch path.

use crate::skills::facade::SkillRegistryContext;

#[must_use]
pub(crate) fn first_party_context() -> SkillRegistryContext {
    SkillRegistryContext::first_party_only()
}
