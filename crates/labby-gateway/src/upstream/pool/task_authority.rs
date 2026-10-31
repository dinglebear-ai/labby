//! Runtime-only credential authority observations for durable task reconciliation.

use std::collections::HashSet;

use labby_runtime::gateway_config::UpstreamConfig;
use sha2::{Digest, Sha256};

use super::UpstreamPool;

fn authority_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn configured_authority(config: &UpstreamConfig) -> Option<[u8; 32]> {
    let bearer = config
        .bearer_token_env
        .as_deref()
        .and_then(crate::upstream::auth::configured_bearer_token);
    authority_with_header_values(
        config,
        bearer.as_deref(),
        crate::upstream::auth::configured_header_value,
    )
}

fn authority_with_header_values(
    config: &UpstreamConfig,
    bearer: Option<&str>,
    header_value: impl Fn(&str) -> Option<String>,
) -> Option<[u8; 32]> {
    // Preserve the existing observation for all upstreams without header refs.
    if config.headers_env.is_empty() {
        return bearer.map(authority_digest);
    }
    let mut digest = Sha256::new();
    digest.update(b"labby.upstream.header-authority.v1\0");
    hash_optional_value(&mut digest, bearer);
    for (name, env_name) in &config.headers_env {
        hash_optional_value(&mut digest, Some(&name.to_ascii_lowercase()));
        let value = header_value(env_name);
        hash_optional_value(&mut digest, value.as_deref());
    }
    // Only this runtime digest survives resolution; credentials never enter the
    // durable task record, configuration fingerprint, or diagnostic output.
    Some(digest.finalize().into())
}

fn hash_optional_value(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        None => digest.update([0]),
    }
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
        let prospective = self.configured_upstreams.get(upstream).map_or_else(
            || token.as_deref().map(authority_digest),
            |config| {
                authority_with_header_values(
                    &config,
                    token.as_deref(),
                    crate::upstream::auth::configured_header_value,
                )
            },
        );
        self.upstream_bearer_authorities
            .get(upstream)
            .is_some_and(|previous| *previous == prospective)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> UpstreamConfig {
        super::super::testsupport::test_upstream_config()
    }

    #[test]
    fn legacy_authority_digest_is_unchanged_without_header_references() {
        let config = config();
        assert_eq!(
            authority_with_header_values(&config, None, |_| unreachable!()),
            None
        );
        assert_eq!(
            authority_with_header_values(&config, Some("bearer-fixture"), |_| unreachable!()),
            Some(authority_digest("bearer-fixture")),
        );
    }

    #[test]
    fn header_authority_detects_rotated_removed_and_blank_values() {
        let mut config = config();
        config
            .headers_env
            .insert("CF-Access-Client-Secret".into(), "CF_SECRET".into());
        let original =
            authority_with_header_values(&config, None, |_| Some("first-fixture".into()));
        assert_eq!(
            original,
            authority_with_header_values(&config, None, |_| Some("first-fixture".into()))
        );
        for value in [Some("rotated-fixture"), None, Some(""), Some("   ")] {
            assert_ne!(
                original,
                authority_with_header_values(&config, None, |_| value.map(str::to_owned))
            );
        }
        assert_ne!(
            authority_with_header_values(&config, None, |_| None),
            authority_with_header_values(&config, None, |_| Some(String::new())),
        );
    }

    #[test]
    fn header_authority_covers_each_header_name_raw_value_and_separate_bearer() {
        let mut config = config();
        config
            .headers_env
            .insert("CF-Access-Client-Id".into(), "CF_ID".into());
        config
            .headers_env
            .insert("CF-Access-Client-Secret".into(), "CF_SECRET".into());
        let resolve = |name: &str| Some(format!("fixture-{name}"));
        let original = authority_with_header_values(&config, Some("bearer"), resolve);
        assert_ne!(
            original,
            authority_with_header_values(&config, Some("other-bearer"), resolve)
        );
        assert_ne!(
            original,
            authority_with_header_values(&config, Some("bearer"), |name| {
                Some(if name == "CF_ID" {
                    "changed-id".into()
                } else {
                    format!("fixture-{name}")
                })
            })
        );
        assert_ne!(
            original,
            authority_with_header_values(&config, Some("bearer"), |name| {
                Some(format!(" fixture-{name} "))
            })
        );
        config.headers_env.remove("CF-Access-Client-Id");
        config
            .headers_env
            .insert("x-different-id".into(), "CF_ID".into());
        assert_ne!(
            original,
            authority_with_header_values(&config, Some("bearer"), resolve)
        );
    }
}
