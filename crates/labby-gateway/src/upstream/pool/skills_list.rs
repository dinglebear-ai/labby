//! Upstream Agent Skills enumeration and retrieval (SEP-2640).
//!
//! Two RPCs with deliberately different bulkhead treatment:
//!
//! - `skills/list` is a **fan-out aggregation pass**, so like the prompt and
//!   resource listings it is a documented exception to the per-upstream permit
//!   and keeps partial-result semantics.
//! - `skills/get` is a **single-target, caller-attributed** call, so it goes
//!   through [`super::capability_call::timed_capability_call_str`] like any other direct access.
//!
//! # Why the pagination loop is hand-written
//!
//! rmcp exposes no cursor helper, and its own `list_all_*` convenience methods
//! accumulate every page with no cap before any limit can engage. A hostile
//! upstream could therefore stream unbounded pages inside the wall-clock
//! budget. Budgets here are enforced *incrementally, per page*: the walk stops
//! the moment the running skill count or the page count crosses its cap, before
//! the next request is issued.
//!
//! # A partial traversal is never a complete snapshot
//!
//! If a page fails mid-walk, this returns the error rather than the pages
//! collected so far. Caching a truncated-by-error walk as if it were the whole
//! catalog would make a later `skill_manifest_stale` unrecoverable: its advice
//! is to re-list, which would just return the same partial cache for a full
//! TTL. Truncation by a *budget* is different — it is deterministic and is
//! reported through `truncated`.

use std::collections::BTreeMap;
use std::time::Instant;

use rmcp::RoleClient;
use rmcp::model::{ClientRequest, CustomRequest};
use rmcp::service::{Peer, ServiceError};
use serde_json::json;

use labby_runtime::skills::wire::{
    SKILLS_EXTENSION_KEY, SKILLS_GET_METHOD, SKILLS_LIST_METHOD, SkillEntry, SkillsGetResult,
    SkillsListResult,
};
use labby_runtime::skills::{
    SkillRejection, ValidatedSkill, limits, parse_skill_resource_uri, validate_skill_entry,
    validate_skill_entry_detailed,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct SkillResourceBinding {
    pub(super) skill: usize,
    pub(super) resource: usize,
}

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::{
    CapabilityCallError, timed_capability_call, timed_capability_call_with_timeout,
};
use super::helpers::redact_resource_uri_for_logging;
use super::logging::{UpstreamRequestLog, log_upstream_request_start};

/// Stable typed failures from the upstream Agent Skills path. Arbitrary peer
/// text remains attached to the nested capability error for local diagnostics,
/// while public Display output is bounded and non-secret.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpstreamSkillsError {
    #[error("upstream skills capability request failed")]
    Capability(#[source] CapabilityCallError),
    #[error("upstream skills pagination changed cache scope")]
    CacheScopeChanged,
    #[error("upstream skill manifest failed validation: {reason}")]
    InvalidManifest { reason: &'static str },
    #[error("upstream skills peer is unavailable")]
    Unavailable,
    #[error("upstream skills cache was invalidated during refresh")]
    Invalidated,
    #[error("upstream skills cache entry disappeared during direct retrieval")]
    CacheMissing,
    #[error("upstream skill URI is invalid")]
    InvalidUri,
    #[error("upstream skill response did not match the requested identity")]
    IdentityMismatch,
    #[error("upstream skill collides with existing manifest ownership")]
    Collision,
    #[error("upstream direct skill cache exceeded its bounded capacity")]
    LimitExceeded,
}

impl UpstreamSkillsError {
    pub(super) fn capability(&self) -> Option<&CapabilityCallError> {
        match self {
            Self::Capability(error) => Some(error),
            _ => None,
        }
    }
}

/// One upstream's validated skills plus what was dropped getting there.
#[derive(Debug, Clone, Default)]
pub(super) struct UpstreamSkills {
    /// Skill candidates observed across the upstream pages fetched for this snapshot,
    /// before host validation or exposure policy is applied.
    pub(super) discovered_count: usize,
    /// Skills that passed ingest validation.
    pub(super) skills: Vec<ValidatedSkill>,
    /// Skills dropped for integrity or budget reasons, by cause. Operators see
    /// the causes; agents see only the total, so a completeness signal never
    /// doubles as a way to enumerate an operator's configuration.
    pub(super) excluded: Vec<ExcludedSkill>,
    /// Whether a budget stopped the walk early. Distinct from an error: this
    /// snapshot is complete as far as it goes and is safe to cache.
    pub(super) truncated: bool,
    /// Smallest `ttlMs` seen across the pages, if any. A snapshot is only as
    /// fresh as its stalest page.
    pub(super) ttl_ms: Option<u64>,
    /// `cacheScope` as reported. Advisory only — Labby shards per subject
    /// regardless, which is stricter than any value here permits.
    pub(super) cache_scope: Option<String>,
    /// Canonical native URI to every manifest owner. Multiple bindings are
    /// retained so reads can fail closed without rescanning the catalog.
    pub(super) resource_index: BTreeMap<String, Vec<SkillResourceBinding>>,
}

#[derive(Debug, Clone)]
pub(super) struct ExcludedSkill {
    pub(super) reason: SkillRejection,
    pub(super) uri: String,
    pub(super) detail: String,
}

impl UpstreamSkills {
    /// Count of skills dropped for integrity or budget reasons.
    pub(super) fn excluded_count(&self) -> usize {
        self.excluded.len()
    }
}

/// Whether a capability failure is the upstream's structured
/// `-32602 Invalid params` answer for `skills/get`.
///
/// Keep this typed all the way through the pool boundary: matching rendered
/// error text can mistake a nested/proxied `-32602` for authoritative absence.
fn is_invalid_params_error(error: &CapabilityCallError) -> bool {
    matches!(
        error,
        CapabilityCallError::Mcp { data, .. }
            if data.code == rmcp::model::ErrorCode::INVALID_PARAMS
    )
}

/// True when the upstream declared the skills extension in its handshake.
///
/// Capability is read from the recorded `initialize` result rather than probed:
/// calling `skills/list` against a server that never declared the extension
/// would be a wasted round trip on every upstream in the catalog.
pub(super) fn peer_declares_skills(peer: &Peer<RoleClient>) -> bool {
    peer.peer_info().is_some_and(|info| {
        info.capabilities
            .extensions
            .as_ref()
            .is_some_and(|extensions| extensions.contains_key(SKILLS_EXTENSION_KEY))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillIngestCap {
    ValidatedSkills,
    Candidates,
}

fn skills_method_error(method: &str, error: &ServiceError) -> String {
    match error {
        ServiceError::ResponseDeserialization(error) => {
            format!("{method} returned a malformed result: {error}")
        }
        error => format!("{method} failed: {error}"),
    }
}

pub(super) fn skills_list_error(error: &ServiceError) -> String {
    skills_method_error(SKILLS_LIST_METHOD, error)
}

pub(super) fn skills_get_error(error: &ServiceError) -> String {
    skills_method_error(SKILLS_GET_METHOD, error)
}

/// Validate and accumulate one page of entries, honoring the per-upstream caps.
///
/// Returns the cap that stopped accumulation, so the caller can stop walking
/// rather than fetching pages whose contents would be discarded.
fn ingest_page(entries: Vec<SkillEntry>, out: &mut UpstreamSkills) -> Option<SkillIngestCap> {
    // Discovery is what the upstream advertised, not what the host later accepts.
    // Count the fetched page before validation so operator status can distinguish
    // discovered candidates from the validated/exposed subset.
    out.discovered_count = out.discovered_count.saturating_add(entries.len());
    for entry in entries {
        let processed_candidates = out.skills.len().saturating_add(out.excluded.len());
        if processed_candidates >= limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM {
            return Some(SkillIngestCap::Candidates);
        }
        if out.skills.len() >= limits::MAX_SKILLS_PER_UPSTREAM {
            return Some(SkillIngestCap::ValidatedSkills);
        }
        let uri = entry.uri.clone();
        match validate_skill_entry_detailed(&entry) {
            Ok(validated) => {
                let skill = out.skills.len();
                if let Some(resources) = &validated.entry.resources {
                    for (resource, item) in resources.iter().enumerate() {
                        if let Ok(uri) = parse_skill_resource_uri(&item.uri) {
                            out.resource_index
                                .entry(uri.to_uri())
                                .or_default()
                                .push(SkillResourceBinding { skill, resource });
                        }
                    }
                }
                out.skills.push(validated);
            }
            // One malformed skill must never sink the upstream: exclude it,
            // record the cause, and keep going.
            Err(rejection) => out.excluded.push(ExcludedSkill {
                reason: rejection.reason,
                uri,
                detail: rejection.detail,
            }),
        }
    }
    None
}

impl UpstreamPool {
    /// Walk an upstream's `skills/list`, validating each page as it arrives.
    ///
    /// Bulkhead exception: a fan-out catalog pass, like the prompt and resource
    /// listings. Per-upstream failures degrade to a returned error that the
    /// caller records against the circuit breaker.
    pub(super) async fn fetch_upstream_skills(
        &self,
        upstream_name: &str,
        peer: &Peer<RoleClient>,
    ) -> Result<UpstreamSkills, UpstreamSkillsError> {
        let mut out = UpstreamSkills::default();
        let mut cursor: Option<String> = None;
        let deadline = Instant::now() + limits::SKILLS_LIST_TIMEOUT;

        for page in 0..limits::MAX_LIST_PAGES {
            if Instant::now() >= deadline {
                out.truncated = true;
                tracing::warn!(
                    upstream = %upstream_name,
                    pages = page,
                    "skills/list exceeded its wall-clock budget — snapshot truncated"
                );
                break;
            }

            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let request =
                ClientRequest::CustomRequest(CustomRequest::new(SKILLS_LIST_METHOD, Some(params)));

            let page_start = Instant::now();
            let remaining = deadline.saturating_duration_since(page_start);
            let event = UpstreamRequestLog::skills_list(upstream_name, false);
            log_upstream_request_start(event);
            let result: SkillsListResult = timed_capability_call_with_timeout(
                self,
                remaining,
                upstream_name,
                UpstreamCapability::Skills,
                event,
                page_start,
                peer.send_request_as(request),
                |_| 0,
                None,
                |error| format!("upstream `{upstream_name}` {}", skills_list_error(error)),
                format!(
                    "upstream `{upstream_name}` skills/list exceeded the {}ms traversal budget",
                    limits::SKILLS_LIST_TIMEOUT.as_millis()
                ),
                None,
            )
            .await
            .map_err(UpstreamSkillsError::Capability)?;

            // A server MUST apply one cacheScope to every page of a list; a
            // change mid-walk means the pages do not describe one listing.
            match (&out.cache_scope, &result.cache_scope) {
                (None, scope) => out.cache_scope = scope.clone(),
                (Some(first), Some(next)) if first != next => {
                    let _ = (first, next);
                    return Err(UpstreamSkillsError::CacheScopeChanged);
                }
                _ => {}
            }
            // Each page carries its own ttlMs and they may differ; the snapshot
            // is only as fresh as its stalest page.
            out.ttl_ms = match (out.ttl_ms, result.ttl_ms) {
                (Some(current), Some(next)) => Some(current.min(next)),
                (current, next) => current.or(next),
            };

            if let Some(cap) = ingest_page(result.skills, &mut out) {
                out.truncated = true;
                match cap {
                    SkillIngestCap::ValidatedSkills => tracing::warn!(
                        upstream = %upstream_name,
                        cap = limits::MAX_SKILLS_PER_UPSTREAM,
                        "upstream published more validated skills than the per-upstream cap — snapshot truncated"
                    ),
                    SkillIngestCap::Candidates => tracing::warn!(
                        upstream = %upstream_name,
                        cap = limits::MAX_SKILL_CANDIDATES_PER_UPSTREAM,
                        "upstream published more skill candidates than the validation cap — snapshot truncated"
                    ),
                }
                break;
            }

            let Some(next) = result.next_cursor else {
                return Ok(out);
            };
            // A cursor that never advances (or advances forever) is bounded by
            // the page cap below; an identical cursor is caught immediately.
            if cursor.as_deref() == Some(next.as_str()) {
                out.truncated = true;
                tracing::warn!(
                    upstream = %upstream_name,
                    "skills/list repeated its pagination cursor — stopping the walk"
                );
                break;
            }
            cursor = Some(next);
        }

        if cursor.is_some() && !out.truncated {
            out.truncated = true;
            tracing::warn!(
                upstream = %upstream_name,
                cap = limits::MAX_LIST_PAGES,
                "skills/list exceeded the page cap — snapshot truncated"
            );
        }
        Ok(out)
    }

    /// Fetch one skill entry by URI.
    ///
    /// This is the path that makes unlisted skills work: SEP-2640 requires a
    /// host to load a skill given only its URI, and says an empty or partial
    /// listing is never proof that a server has no skills. Caller-attributed
    /// and single-target, so it takes the per-upstream permit.
    ///
    /// `Ok(None)` means the server answered `-32602` — the only response that
    /// means "not a skill I serve". Every other failure is `Err`.
    pub(super) async fn fetch_upstream_skill(
        &self,
        upstream_name: &str,
        peer: &Peer<RoleClient>,
        uri: &str,
        subject: Option<&str>,
    ) -> Result<Option<ValidatedSkill>, UpstreamSkillsError> {
        let start = Instant::now();
        // Redacted before it reaches a log line, like every other URI-shaped
        // item on this path.
        let redacted_uri = redact_resource_uri_for_logging(uri);
        let event = UpstreamRequestLog::skill(upstream_name, redacted_uri, subject.is_some());
        log_upstream_request_start(event);

        let request = ClientRequest::CustomRequest(CustomRequest::new(
            SKILLS_GET_METHOD,
            Some(json!({ "uri": uri })),
        ));
        let timeout_ms = self.request_timeout.as_millis();
        let result = timed_capability_call(
            self,
            upstream_name,
            UpstreamCapability::Skills,
            event,
            start,
            peer.send_request_as::<SkillsGetResult>(request),
            |_| 0,
            subject,
            |error| format!("upstream `{upstream_name}` {}", skills_get_error(error)),
            format!("upstream `{upstream_name}` skills/get timed out after {timeout_ms}ms"),
        )
        .await;

        let parsed = match result {
            Ok(result) => result,
            Err(error) if is_invalid_params_error(&error) => return Ok(None),
            Err(error) => return Err(UpstreamSkillsError::Capability(error)),
        };

        validate_skill_entry(&parsed.skill)
            .map(Some)
            .map_err(|reason| UpstreamSkillsError::InvalidManifest {
                reason: reason.as_str(),
            })
    }
}
