//! `expose_prompts` enforcement for the merged prompt catalog.
//!
//! Split out of `prompts_list.rs` to keep that module under the 500-LOC rule.
//! The policy itself is compiled once per upstream in `pool/entries.rs`; this
//! module only applies it to an already-merged prompt list.

use std::collections::HashMap;

use rmcp::model::Prompt;

use super::super::types::ToolExposurePolicy;
use super::UpstreamPool;
use super::entries::{log_exposure_filter, prompt_exposed};

impl UpstreamPool {
    /// Drop merged prompts their owning upstream's `expose_prompts` hides.
    ///
    /// Prompts arrive here in the gateway-namespaced `{upstream}/{name}` form
    /// and `owners` maps each back to its upstream, so one catalog read
    /// resolves every policy. An owner with no catalog entry fails closed.
    pub(super) async fn retain_exposed_prompts(
        &self,
        prompts: Vec<Prompt>,
        owners: &HashMap<String, String>,
    ) -> Vec<Prompt> {
        let policies: HashMap<String, ToolExposurePolicy> = {
            let catalog = self.catalog.read().await;
            catalog
                .iter()
                .map(|(name, entry)| (name.clone(), entry.prompt_exposure_policy.clone()))
                .collect()
        };
        let mut hidden: HashMap<String, usize> = HashMap::new();
        let mut exposed: HashMap<String, usize> = HashMap::new();
        let retained = prompts
            .into_iter()
            .filter(|prompt| {
                let Some(owner) = owners.get(prompt.name.as_str()) else {
                    // Builtin/unowned prompts are not upstream-exposed content.
                    return true;
                };
                let allowed = policies
                    .get(owner)
                    .is_some_and(|policy| prompt_exposed(policy, owner, &prompt.name));
                let counter = if allowed { &mut exposed } else { &mut hidden };
                *counter.entry(owner.clone()).or_default() += 1;
                allowed
            })
            .collect();
        for (upstream, hidden_count) in hidden {
            let exposed_count = exposed.get(&upstream).copied().unwrap_or_default();
            log_exposure_filter(&upstream, "prompts", hidden_count, exposed_count, false);
        }
        retained
    }
}
