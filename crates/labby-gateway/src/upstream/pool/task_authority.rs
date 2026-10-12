//! Runtime-only bearer authority observations for durable task reconciliation.

use std::collections::HashSet;

use labby_runtime::gateway_config::UpstreamConfig;
use sha2::{Digest, Sha256};

use super::UpstreamPool;

fn authority_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn configured_authority(config: &UpstreamConfig) -> Option<[u8; 32]> {
    config
        .bearer_token_env
        .as_deref()
        .and_then(crate::upstream::auth::configured_bearer_token)
        .as_deref()
        .map(authority_digest)
}

impl UpstreamPool {
    pub(super) fn seed_task_bearer_authority(&self, config: &UpstreamConfig) {
        self.upstream_bearer_authorities
            .entry(config.name.clone())
            .or_insert_with(|| configured_authority(config));
    }

    pub(super) fn replace_task_bearer_authority(&self, config: &UpstreamConfig) {
        self.upstream_bearer_authorities
            .insert(config.name.clone(), configured_authority(config));
    }

    pub(super) fn task_bearer_authority_matches(&self, config: &UpstreamConfig) -> bool {
        self.upstream_bearer_authorities
            .get(&config.name)
            .is_some_and(|previous| *previous == configured_authority(config))
    }

    pub(crate) fn task_bearer_authority_matches_token(
        &self,
        upstream: &str,
        token: Option<String>,
    ) -> bool {
        self.upstream_bearer_authorities
            .get(upstream)
            .is_some_and(|previous| *previous == token.as_deref().map(authority_digest))
    }

    pub(crate) fn task_bearer_authority_changed_names(
        &self,
        configs: &[UpstreamConfig],
    ) -> HashSet<String> {
        configs
            .iter()
            .filter(|config| {
                self.configured_upstreams.contains_key(&config.name)
                    && !self.task_bearer_authority_matches(config)
            })
            .map(|config| config.name.clone())
            .collect()
    }
}
