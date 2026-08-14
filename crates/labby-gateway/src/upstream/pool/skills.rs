//! The pool-facing entry point for upstream Agent Skills (SEP-2640).
//!
//! Composes the three layers beneath it: the opt-in `proxy_skills` gate, the
//! per-`(upstream, subject)` cache, and the `expose_skills` allowlist. Callers
//! get a filtered, cached snapshot and never touch the wire directly.
//!
//! # The exposure gate runs on read, not only on fetch
//!
//! The cache holds the *unfiltered* catalog and the allowlist is applied on
//! every read. That ordering matters: an operator narrowing `expose_skills`
//! must take effect immediately, not after a TTL, and a cache populated under
//! one policy must never keep serving under it once the policy changes.

use std::collections::BTreeSet;

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{ValidatedSkill, parse_skill_uri};

use super::UpstreamPool;
use super::entries::{log_exposure_filter, resolve_request_skill_exposure_policy};
use super::skills_cache::{CachedSkills, evict};
use std::time::Instant;

use super::capability_call::timed_capability_call_str;
use super::logging::{UpstreamRequestLog, log_upstream_request_start};
use super::skills_list::{UpstreamSkills, peer_declares_skills};

/// One upstream's exposed skills, plus the completeness bookkeeping a caller
/// needs to report honestly.
#[derive(Debug, Clone, Default)]
pub struct ExposedSkills {
    /// Skills this caller may see, after the allowlist.
    pub skills: Vec<ValidatedSkill>,
    /// Skills dropped for integrity or budget reasons. Surfaced to agents as a
    /// bare count and to operators in full — never as a per-skill list to a
    /// downstream caller, which would leak the shape of an operator's config.
    pub excluded_count: usize,
    /// Whether a budget cut the upstream walk short.
    pub truncated: bool,
    /// Age of the underlying snapshot, for operator display.
    pub age_secs: u64,
    /// Remaining lifetime of this snapshot, clamped from the upstream's
    /// untrusted `ttlMs`. A downstream listing that folds these entries in must
    /// not advertise a longer TTL than the data behind it actually has.
    pub ttl_ms: Option<u64>,
}

impl UpstreamPool {
    /// Exposed skills for one upstream, fetching or refreshing as needed.
    ///
    /// Returns an empty set — never an error — when the upstream does not
    /// proxy skills or never declared the extension. Neither is a failure, and
    /// treating them as one would put phantom failures on the circuit breaker
    /// for every non-skills upstream in the catalog.
    pub async fn upstream_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<ExposedSkills, String> {
        if !config.proxy_skills {
            return Ok(ExposedSkills::default());
        }
        let key = (config.name.clone(), subject.map(str::to_string));

        // Serve a cached snapshot when one is fresh. An expired snapshot is
        // still served; the refresh happens behind it rather than in front.
        if let Some(cached) = self.cached_skills(&key).await {
            if cached.is_fresh() {
                return Ok(self.apply_skill_exposure(config, &cached, subject));
            }
            let stale = self.apply_skill_exposure(config, &cached, subject);
            self.spawn_skills_refresh(config.clone(), subject.map(str::to_string));
            return Ok(stale);
        }

        // Cold: one caller fetches while the rest wait on the same guard, so a
        // burst of downstream listings makes one upstream request.
        let guard = self.skills_fetch_locks.guard_for(&key).await;
        let _held = guard.lock().await;
        if let Some(cached) = self.cached_skills(&key).await {
            return Ok(self.apply_skill_exposure(config, &cached, subject));
        }

        let snapshot = self.fetch_and_cache_skills(config, subject).await?;
        Ok(self.apply_skill_exposure(config, &snapshot, subject))
    }

    /// Read a cached snapshot, marking it used for idle eviction.
    async fn cached_skills(&self, key: &(String, Option<String>)) -> Option<CachedSkills> {
        let mut cache = self.skills_cache.write().await;
        let entry = cache.get_mut(key)?;
        entry.touch();
        Some(entry.clone())
    }

    /// Fetch one upstream's catalog and store it.
    async fn fetch_and_cache_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<CachedSkills, String> {
        // Upstreams connect lazily: a cold gateway has a seeded catalog entry but
        // no live connection until something asks for one, so acquiring the peer
        // directly reports "not connected" on every first read — the normal
        // state for `labby mcp`, not an error.
        //
        // Gated on the connection actually being absent, not on the upstream
        // having healthy tools. `ensure_tools_for_upstream` tears down and
        // reconnects whenever an upstream has no healthy tools, and a
        // skills-only upstream never has any — routing through it
        // unconditionally would reconnect on every single read.
        let connected = self.connections.read().await.contains_key(&config.name);
        if !connected
            && let Err(error) = self.ensure_tools_for_upstream(config, subject, None).await
        {
            return Err(format!(
                "upstream `{}` could not be connected for skills: {error}",
                config.name
            ));
        }
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skills.list",
            )
            .await
            .ok_or_else(|| format!("upstream `{}` is not connected", config.name))?;

        // An upstream that never declared the extension is not a failure — it
        // simply has no skills, and caching that avoids re-asking every read.
        if !peer_declares_skills(&peer) {
            let empty = CachedSkills::new(UpstreamSkills::default());
            self.store_skills(&config.name, subject, empty.clone())
                .await;
            return Ok(empty);
        }

        match self.fetch_upstream_skills(&config.name, &peer).await {
            Ok(skills) => {
                let count = skills.skills.len();
                let excluded = skills.excluded.clone();
                let entry = CachedSkills::new(skills);
                self.store_skills(&config.name, subject, entry.clone())
                    .await;
                self.record_success_for(
                    &config.name,
                    super::super::types::UpstreamCapability::Skills,
                )
                .await;
                {
                    let mut catalog = self.catalog.write().await;
                    if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                        catalog_entry.skill_count = count;
                    }
                }
                for (reason, uri) in &excluded {
                    tracing::warn!(
                        upstream = %config.name,
                        reason = reason.as_str(),
                        skill = %super::helpers::redact_resource_uri_for_logging(uri),
                        "excluded an upstream skill at ingest"
                    );
                }
                Ok(entry)
            }
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    super::super::types::UpstreamCapability::Skills,
                    format!("failed to list skills from upstream: {error}"),
                )
                .await;
                Err(error)
            }
        }
    }

    async fn store_skills(&self, name: &str, subject: Option<&str>, entry: CachedSkills) {
        let mut cache = self.skills_cache.write().await;
        cache.insert((name.to_string(), subject.map(str::to_string)), entry);
        evict(&mut cache);
    }

    /// Refresh an expired snapshot behind the caller.
    ///
    /// Marks the entry refreshing first so a burst of readers past the TTL
    /// spawns one task rather than one per reader.
    fn spawn_skills_refresh(&self, config: UpstreamConfig, subject: Option<String>) {
        let pool = self.clone();
        tokio::spawn(async move {
            let key = (config.name.clone(), subject.clone());
            {
                let mut cache = pool.skills_cache.write().await;
                match cache.get_mut(&key) {
                    Some(entry) if entry.refreshing => return,
                    Some(entry) => entry.refreshing = true,
                    None => return,
                }
            }
            let result = pool
                .fetch_and_cache_skills(&config, subject.as_deref())
                .await;
            if result.is_err() {
                // Keep serving the stale snapshot; clear the flag so a later
                // read can try again rather than pinning it forever.
                let mut cache = pool.skills_cache.write().await;
                if let Some(entry) = cache.get_mut(&key) {
                    entry.refreshing = false;
                }
            }
            pool.skills_fetch_locks.prune().await;
        });
    }

    /// Apply `expose_skills` to a cached snapshot.
    fn apply_skill_exposure(
        &self,
        config: &UpstreamConfig,
        cached: &CachedSkills,
        subject: Option<&str>,
    ) -> ExposedSkills {
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let total = cached.skills.skills.len();
        let skills: Vec<ValidatedSkill> = cached
            .skills
            .skills
            .iter()
            .filter(|skill| policy.matches(&skill.name))
            .cloned()
            .collect();
        log_exposure_filter(
            &config.name,
            "skills",
            total - skills.len(),
            skills.len(),
            subject.is_some(),
        );
        ExposedSkills {
            skills,
            excluded_count: cached.skills.excluded_count(),
            truncated: cached.skills.truncated,
            age_secs: cached.age().as_secs(),
            ttl_ms: Some(cached.remaining_ttl().as_millis() as u64),
        }
    }

    /// Fetch one skill by URI from an upstream that did not list it.
    ///
    /// SEP-2640 requires a host to load a skill given only its URI, and says an
    /// empty or partial listing is never proof a server has no skills. Without
    /// this, a skill absent from a cached or budget-truncated listing is
    /// permanently unreachable even though the upstream would serve it.
    ///
    /// Still gated: the upstream must opt in, and the returned entry passes the
    /// same ingest validation and `expose_skills` allowlist a listed skill does,
    /// so unlisted does not mean unfiltered.
    pub async fn fetch_unlisted_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        uri: &str,
    ) -> Option<ValidatedSkill> {
        if !config.proxy_skills {
            return None;
        }
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skills.get",
            )
            .await?;
        if !peer_declares_skills(&peer) {
            return None;
        }
        let skill = match self
            .fetch_upstream_skill(&config.name, &peer, uri, subject)
            .await
        {
            Ok(skill) => skill?,
            Err(error) => {
                tracing::warn!(
                    upstream = %config.name,
                    error = %error,
                    "skills/get for an unlisted skill failed"
                );
                return None;
            }
        };

        // The allowlist applies to a skill fetched by URI exactly as it does to
        // a listed one; filtering only the listing would be a bypass.
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        policy.matches(&skill.name).then_some(skill)
    }

    /// Drop every cached skill catalog for one upstream, across all subjects.
    ///
    /// Called on reload and on disconnect: a snapshot outliving the connection
    /// it came from would serve a catalog Labby can no longer honor a read
    /// against.
    pub async fn invalidate_upstream_skills(&self, name: &str) {
        let mut cache = self.skills_cache.write().await;
        cache.retain(|(upstream, _), _| upstream != name);
    }

    /// Drop every cached skill catalog, across all upstreams and subjects.
    ///
    /// Called on pool drain (config reload / swap). Skills are cached against a
    /// connection and a config; when both are replaced wholesale, so is the
    /// cache.
    pub async fn clear_all_cached_skills(&self) {
        let mut cache = self.skills_cache.write().await;
        let count = cache.len();
        cache.clear();
        drop(cache);
        self.skills_fetch_locks.prune().await;
        if count > 0 {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                cleared = count,
                "cleared cached upstream skill catalogs"
            );
        }
    }

    /// Names of upstreams with a cached skill catalog, for operator display.
    pub async fn upstreams_with_cached_skills(&self) -> BTreeSet<String> {
        self.skills_cache
            .read()
            .await
            .keys()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Bytes of one proxied skill file, verified against the manifest that
/// published it.
#[derive(Debug, Clone)]
pub struct VerifiedSkillFile {
    pub text: String,
    pub mime_type: Option<String>,
}

impl UpstreamPool {
    /// Read one file of a proxied skill, by the path Labby minted for it.
    ///
    /// `path` is the URI remainder after Labby's origin label. The upstream
    /// knows the file under its *own* origin, so the cached entry is what maps
    /// one to the other — Labby never guesses the upstream's label.
    ///
    /// Every read is manifest-bound and digest-verified. A URI the manifest does
    /// not list is refused rather than fetched: the SEP treats an unlisted file
    /// within a skill as a change to the skill, equivalent to a digest mismatch.
    pub async fn read_proxied_skill_file(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        // The upstream's OWN full `<skill-path>/<file-path>` — i.e. what
        // remains after stripping the origin label Labby prepended. Named for
        // the invariant because passing a label-relative remainder here silently
        // matches nothing.
        upstream_path: &str,
    ) -> Result<VerifiedSkillFile, ToolError> {
        let path = upstream_path;
        let exposed = self
            .upstream_skills(config, subject)
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: error,
            })?;

        // `path` is the remainder after the label Labby prepended, which is
        // exactly the upstream's own full path — so it is matched against the
        // upstream URI's *full* path, not its remainder. Matching remainders
        // would drop the upstream's own first segment on both sides and make
        // `skill://git-workflow/SKILL.md` (a legal one-segment skill path)
        // unmatchable.
        //
        // The owning skill travels with the match: verifying a `SKILL.md`
        // needs the frontmatter its own entry published, not another skill's.
        let mut found: Option<(&str, &str, &ValidatedSkill)> = None;
        for skill in &exposed.skills {
            let Some(resources) = skill.entry.resources.as_ref() else {
                continue;
            };
            for resource in resources {
                if let Ok(parsed) = parse_skill_uri(&resource.uri)
                    && parsed.full_path() == path
                {
                    // Two schemes can share a path — `skill://a/SKILL.md` and
                    // `github://a/SKILL.md` both publish as
                    // `skill://<label>/a/SKILL.md`. Resolving by iteration order
                    // would serve one skill's bytes under the other's identity,
                    // so an ambiguous path is refused instead.
                    if let Some((existing, _, _)) = found
                        && existing != resource.uri.as_str()
                    {
                        return Err(ToolError::Sdk {
                            sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
                            message: format!(
                                "`{path}` on upstream `{}` is served under more than one scheme, \
                                 so it does not identify a single file",
                                config.name
                            ),
                        });
                    }
                    found = Some((resource.uri.as_str(), resource.digest.as_str(), skill));
                }
            }
        }

        let Some((upstream_uri, digest, skill)) = found else {
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
                message: format!(
                    "no exposed skill on upstream `{}` lists `{path}`; refresh the skill entry and retry",
                    config.name
                ),
            });
        };

        // Read from the owning upstream directly, preserving its native
        // `skill://` URI. The URI-routed helpers resolve a `lab://upstream/…`
        // gateway namespace and cannot place a native skill URI.
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skill.read",
            )
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: format!("upstream `{}` is not connected", config.name),
            })?;

        let start = Instant::now();
        let redacted = super::helpers::redact_resource_uri_for_logging(upstream_uri);
        let event = UpstreamRequestLog::skill(&config.name, redacted, subject.is_some());
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let contents = timed_capability_call_str(
            self,
            &config.name,
            super::super::types::UpstreamCapability::Skills,
            event,
            start,
            peer.read_resource(rmcp::model::ReadResourceRequestParams::new(upstream_uri)),
            |_| 0,
            subject,
            |error| format!("upstream `{}` skill read failed: {error}", config.name),
            format!(
                "upstream `{}` skill read timed out after {timeout_ms}ms",
                config.name
            ),
        )
        .await
        .map_err(|message| ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message,
        })?;

        // Exactly one content block: the digest model is one URI to one blob.
        let [content] = contents.contents.as_slice() else {
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "upstream `{}` returned {} content blocks for one skill file",
                    config.name,
                    contents.contents.len()
                ),
            });
        };
        let (text, mime_type) = match content {
            rmcp::model::ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => (text.clone(), mime_type.clone()),
            // Anything else — a blob today, a new variant tomorrow — cannot be
            // digest-verified as text, so it is refused rather than relayed.
            _ => {
                return Err(ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: "skill files are served as text; this content cannot be verified"
                        .to_string(),
                });
            }
        };

        let parsed_digest =
            labby_runtime::skills::parse_digest(digest).map_err(|error| ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "upstream `{}` published an unusable digest: {error}",
                    config.name
                ),
            })?;
        if !parsed_digest.matches(text.as_bytes()) {
            // Zero bytes reach the caller. A digest match is a consistency
            // check, not proof of trustworthiness, but a mismatch is proof of
            // inconsistency and the content must not be used.
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "content of `{path}` from upstream `{}` does not match the digest its entry published",
                    config.name
                ),
            });
        }

        // A digest match does not subsume the frontmatter check, and this is
        // the gap that check exists to close: an upstream may publish benign
        // `frontmatter` in its `skills/list` entry while the real `SKILL.md`
        // body carries something else — `allowed-tools: ["*"]`, say. The digest
        // is computed over the real body, so it matches, and the body a client
        // acts on grants capabilities the entry a user approved never declared.
        // That is threat-model T3, and only a field-by-field comparison of the
        // served bytes against the published entry catches it.
        if is_skill_md(&skill.entry.uri, path) {
            let served =
                labby_runtime::skills::parse_skill_md_frontmatter(&text).map_err(|error| {
                    ToolError::Sdk {
                        sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                        message: format!(
                            "`SKILL.md` from upstream `{}` has unparseable frontmatter: {error}",
                            config.name
                        ),
                    }
                })?;
            labby_runtime::skills::compare_frontmatter(&skill.entry.frontmatter, &served).map_err(
                |error| ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: format!(
                        "`SKILL.md` from upstream `{}` disagrees with the frontmatter its entry published: {error}",
                        config.name
                    ),
                },
            )?;
        }

        Ok(VerifiedSkillFile { text, mime_type })
    }
}

/// Whether `path` addresses the `SKILL.md` of the skill rooted at `entry_uri`.
///
/// Compared against the entry's own URI rather than by matching the `SKILL.md`
/// suffix, so a nested `references/SKILL.md` is not mistaken for the skill's
/// own definition and cross-verified against the wrong frontmatter.
fn is_skill_md(entry_uri: &str, upstream_path: &str) -> bool {
    // Compared on the full path, matching what the caller passes. Using the
    // post-first-segment remainder here made this silently return false for
    // every skill, which disabled the frontmatter cross-check entirely — a
    // security check that fails open by never firing.
    parse_skill_uri(entry_uri).is_ok_and(|parsed| parsed.full_path() == upstream_path)
}
