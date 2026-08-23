//! Coherent, generation-bearing views of the published gateway runtime.

use std::sync::atomic::{AtomicU64, Ordering};

use labby_runtime::gateway_config::GatewayLoadoutConfig;

use super::GatewayManager;

static NEXT_RUNTIME_CONFIG_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_runtime_config_generation() -> u64 {
    NEXT_RUNTIME_CONFIG_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            generation.checked_add(1)
        })
        .expect("gateway runtime-config generation exhausted")
}

/// Opaque process-local identity of a published gateway runtime configuration
/// revision.
///
/// Callers may compare generations for equality. The numeric representation is
/// deliberately private: it is not durable state and must not be synthesized
/// from config content. In particular, an A -> B -> A publication produces
/// three distinct generations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayRuntimeConfigGeneration(u64);

/// A Loadout resolved only from the runtime configuration published by this
/// process, paired with the exact publication generation that supplied it.
///
/// This never reads the durable desired configuration, where restart-bound
/// Loadout edits may be staged but not active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedRuntimeLoadoutSnapshot {
    generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
}

impl PublishedRuntimeLoadoutSnapshot {
    #[must_use]
    pub fn generation(&self) -> GatewayRuntimeConfigGeneration {
        self.generation
    }

    #[must_use]
    pub fn loadout(&self) -> Option<&GatewayLoadoutConfig> {
        self.loadout.as_ref()
    }

    #[must_use]
    pub fn into_loadout(self) -> Option<GatewayLoadoutConfig> {
        self.loadout
    }
}

impl GatewayManager {
    /// Resolve a named Loadout from one coherent published runtime revision.
    pub async fn published_runtime_loadout_snapshot(
        &self,
        name: &str,
    ) -> PublishedRuntimeLoadoutSnapshot {
        let _publication = self.publication_barrier.read().await;
        let loadout = self
            .config
            .read()
            .await
            .loadouts
            .iter()
            .find(|loadout| loadout.name == name)
            .cloned();
        let generation =
            GatewayRuntimeConfigGeneration(self.runtime_config_generation.load(Ordering::Relaxed));
        PublishedRuntimeLoadoutSnapshot {
            generation,
            loadout,
        }
    }

    /// Advance the runtime-config publication identity while the caller holds
    /// the publication writer. Pool/catalog-only mutations are deliberately
    /// outside this Loadout snapshot contract. Overflow is a process-fatal
    /// invariant breach rather than an ABA collision.
    pub(super) fn advance_runtime_config_generation(&self) {
        self.runtime_config_generation
            .store(next_runtime_config_generation(), Ordering::Relaxed);
    }
}
