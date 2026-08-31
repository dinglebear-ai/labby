//! Authentication middleware helpers shared by the top-level API router.

use std::sync::Arc;

/// Adapt the product's typed actor-key derivation to `labby-auth`'s erased
/// callback without coupling the auth crate to product observability types.
pub(super) fn lab_auth_deriver(
    deriver: Arc<crate::observability::activity::ActorKeyDeriver>,
) -> Arc<labby_auth::ActorKeyDeriver> {
    Arc::new(move |subject: &str| {
        deriver
            .derive_subject(subject)
            .map(crate::observability::activity::ActorKey::into_arc)
    })
}

pub(super) fn derive_actor_key(
    deriver: Option<&crate::observability::activity::ActorKeyDeriver>,
    subject: &str,
) -> Option<Arc<str>> {
    deriver
        .and_then(|deriver| deriver.derive_subject(subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}
