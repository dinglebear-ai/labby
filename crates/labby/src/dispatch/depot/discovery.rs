//! Deterministic bounded merge and wire projection for Depot discovery.
use crate::config::depot::MAX_SAFE_INTEGER;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::time::Instant;

use super::cursor::{Binding, CursorError, PageInput};
use super::health::Failure;
use super::manager::Manager;
use super::network::Operation;
use super::provider::ProviderError;
use futures::future::join_all;

const MAX_PAGE: u16 = 200;
const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_FIELD: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryFeed {
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceOrigin {
    McpRegistry,
    AcpRegistry,
    Ard,
}

impl SourceOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::McpRegistry => "mcp-registry",
            Self::AcpRegistry => "acp-registry",
            Self::Ard => "ard",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("query must be empty or contain 3 to 200 characters")]
    InvalidQuery,
    #[error("page limit must be from 1 to 200")]
    InvalidLimit,
    #[error("Depot provider returned an incompatible discovery result")]
    InvalidProvider,
    #[error("federated response exceeds its byte limit")]
    ResponseTooLarge,
    #[error("selected Depot provider is unavailable")]
    ProviderUnavailable,
    #[error("discovery cursor expired; restart")]
    CursorExpired,
    #[error("Depot discovery is at capacity")]
    Capacity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRequest {
    #[serde(default)]
    pub source_origin: Option<SourceOrigin>,
    #[serde(default)]
    pub feed: Option<DiscoveryFeed>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u16,
    #[serde(default)]
    pub cursor: Option<String>,
}

const fn default_limit() -> u16 {
    50
}

pub async fn discover(
    manager: &Manager,
    authority: &labby_auth::browser_authority::BrowserAuthority,
    request: &DiscoveryRequest,
    receipt: tokio::time::Instant,
) -> Result<DiscoveryResponse, DiscoveryError> {
    discover_with_access_epoch(manager, authority, request, receipt, None).await
}

/// The API supplies a freshly authorized project epoch, never a client parameter.
pub(crate) async fn discover_with_access_epoch(
    manager: &Manager,
    authority: &labby_auth::browser_authority::BrowserAuthority,
    request: &DiscoveryRequest,
    receipt: tokio::time::Instant,
    access_epoch: Option<&str>,
) -> Result<DiscoveryResponse, DiscoveryError> {
    validate_request(&request.query, request.limit)?;
    let topology = manager.snapshot();
    let selected: Vec<_> = topology
        .providers
        .values()
        .filter(|provider| provider.view.enabled)
        .filter(|provider| {
            request
                .provider
                .as_ref()
                .is_none_or(|id| id == &provider.view.id)
        })
        .cloned()
        .collect();
    if request.provider.is_some() && selected.is_empty() {
        return Err(DiscoveryError::ProviderUnavailable);
    }
    let scope = request.provider.clone().unwrap_or_else(|| "all".into());
    let registry_epoch = request.provider.as_ref().map_or_else(
        || topology.membership_epoch.clone(),
        |_| format!("selected:{scope}"),
    );
    let current = selected
        .iter()
        .map(|provider| {
            (
                provider.view.id.clone(),
                provider.runtime.incarnation().to_owned(),
                String::new(),
            )
        })
        .collect();
    let mut current_binding = Binding::for_browser(
        authority,
        "lab:read",
        scope.clone(),
        request.query.clone(),
        page_contract(request),
        registry_epoch.clone(),
        current,
    )
    .await?;
    current_binding.bind_access_epoch(access_epoch);
    let now = Instant::now();
    let (input, stored_binding) = if let Some(cursor) = &request.cursor {
        (
            manager.cursors.begin(cursor, &current_binding, now).await?,
            current_binding,
        )
    } else {
        let admission = manager
            .scheduler
            .admit(&current_binding.actor, receipt)
            .await
            .map_err(|_| DiscoveryError::Capacity)?;
        let qualified = join_all(selected.iter().map(|provider| {
            let admission = &admission;
            async move { (provider, provider.runtime.qualify(admission, false).await) }
        }))
        .await;
        let mut federation = Federation {
            start: 0,
            as_of: request.feed.map(|_| {
                // Depot timestamps have microsecond precision. A nanosecond
                // cutoff would be truncated upstream and fail exact matching.
                jiff::Timestamp::from_microsecond(jiff::Timestamp::now().as_microsecond())
                    .expect("current timestamp is representable")
                    .to_string()
            }),
            ranked_started: false,
            providers: qualified
                .into_iter()
                .map(|(provider, identity)| provider_state(provider, identity))
                .collect(),
        };
        federation.providers.sort_by(|a, b| a.id.cmp(&b.id));
        let providers = federation
            .providers
            .iter()
            .map(|provider| {
                (
                    provider.id.clone(),
                    provider.incarnation.clone(),
                    provider.listing_epoch.clone(),
                )
            })
            .collect();
        let mut binding = Binding::for_browser(
            authority,
            "lab:read",
            scope,
            request.query.clone(),
            page_contract(request),
            registry_epoch,
            providers,
        )
        .await?;
        binding.bind_access_epoch(access_epoch);
        let bytes = serde_json::to_vec(&federation).map_err(|_| DiscoveryError::InvalidProvider)?;
        let cursor = manager.cursors.create(binding.clone(), bytes, now).await?;
        (
            manager.cursors.begin(&cursor, &binding, now).await?,
            binding,
        )
    };
    if let PageInput::Replay(page) = input {
        let mut response: DiscoveryResponse =
            serde_json::from_slice(&page.response).map_err(|_| DiscoveryError::CursorExpired)?;
        response.next_cursor = page.next_cursor;
        return Ok(response);
    }
    let PageInput::Compute(lease) = input else {
        unreachable!()
    };
    let mut federation: Federation =
        serde_json::from_slice(lease.state()).map_err(|_| DiscoveryError::CursorExpired)?;
    let admission = manager
        .scheduler
        .admit(&stored_binding.actor, receipt)
        .await
        .map_err(|_| DiscoveryError::Capacity)?;
    let ranked_items = if request.feed == Some(DiscoveryFeed::New) {
        Some(fetch_new_page(&selected, &mut federation, request, &admission).await?)
    } else {
        fetch_pages(&selected, &mut federation, request, &admission).await?;
        None
    };
    let mut pages: Vec<_> = federation
        .providers
        .iter()
        .map(|provider| provider.page.clone())
        .collect();
    let mut response = if let Some(items) = ranked_items {
        let mut summaries = pages.clone();
        for page in &mut summaries {
            page.items.clear();
        }
        let mut response = with_ranked_items(merge_page(&mut summaries, 0, request.limit)?, items);
        response.feed = request.feed;
        response.as_of = federation.as_of.clone();
        response.ranking_version = Some("new/v1".into());
        response.feed_coverage = Some(federation.providers.iter().map(|provider| {
            serde_json::json!({"providerId": provider.id, "coverage": provider.new_coverage})
        }).collect());
        response
    } else {
        merge_page(&mut pages, federation.start, request.limit)?
    };
    response.scope = stored_binding.scope.clone();
    response.source_origin = request.source_origin;
    response.scope_epoch = stored_binding.authority_epoch.clone();
    pages.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
    for provider in &mut federation.providers {
        if let Some(page) = pages.iter().find(|page| page.provider_id == provider.id) {
            provider.page = page.clone();
        }
    }
    federation.start = if federation.providers.is_empty() {
        0
    } else {
        (federation.start + 1) % federation.providers.len()
    };
    let continuation = federation.providers.iter().any(|provider| {
        !provider.page.items.is_empty()
            || provider.upstream_cursor.is_some()
            || provider.page.outcome == "pending"
    });
    let state = continuation
        .then(|| serde_json::to_vec(&federation))
        .transpose()
        .map_err(|_| DiscoveryError::InvalidProvider)?;
    let bytes = serde_json::to_vec(&response).map_err(|_| DiscoveryError::InvalidProvider)?;
    if bytes.len() > MAX_RESPONSE {
        return Err(DiscoveryError::ResponseTooLarge);
    }
    let page = lease.complete(bytes, state, now).await?;
    response.next_cursor = page.next_cursor;
    Ok(response)
}

fn with_ranked_items(mut response: DiscoveryResponse, items: Vec<Value>) -> DiscoveryResponse {
    response.items = items;
    if response.state == "empty" && !response.items.is_empty() {
        response.state = "complete".into();
    }
    response
}

pub(super) fn page_contract(request: &DiscoveryRequest) -> String {
    let base = match request.feed {
        Some(DiscoveryFeed::New) => format!("discovery/new/v1:{}", request.limit),
        None => format!("discovery/v1:{}", request.limit),
    };
    request.source_origin.map_or_else(
        || base.clone(),
        |origin| format!("{base}:source-origin/v1:{}", origin.as_str()),
    )
}

fn provider_state(
    provider: &super::manager::Provider,
    identity: Result<super::provider::Identity, ProviderError>,
) -> FederatedProvider {
    match identity {
        Ok(identity) => FederatedProvider {
            id: provider.view.id.clone(),
            incarnation: provider.runtime.incarnation().to_owned(),
            listing_epoch: identity.listing_epoch.into(),
            max_page_size: Some(identity.max_page_size),
            upstream_cursor: None,
            page: ProviderPage::participating(&provider.view.id, Vec::new(), None, None),
            new_batch_size: None,
            new_last_key: None,
            new_coverage: None,
        },
        Err(ProviderError::Pending) => FederatedProvider {
            id: provider.view.id.clone(),
            incarnation: provider.runtime.incarnation().to_owned(),
            listing_epoch: String::new(),
            max_page_size: None,
            upstream_cursor: None,
            page: ProviderPage::pending(&provider.view.id),
            new_batch_size: None,
            new_last_key: None,
            new_coverage: None,
        },
        Err(error) => FederatedProvider {
            id: provider.view.id.clone(),
            incarnation: provider.runtime.incarnation().to_owned(),
            listing_epoch: String::new(),
            max_page_size: None,
            upstream_cursor: None,
            page: ProviderPage::failed(&provider.view.id, failure_kind(error)),
            new_batch_size: None,
            new_last_key: None,
            new_coverage: None,
        },
    }
}

async fn fetch_pages(
    selected: &[super::manager::Provider],
    federation: &mut Federation,
    request: &DiscoveryRequest,
    admission: &super::scheduler::Admission,
) -> Result<(), DiscoveryError> {
    // A buffered page is not a capability or authorization lease.
    if let Some(origin) = request.source_origin {
        for state in &federation.providers {
            let provider = selected
                .iter()
                .find(|provider| provider.view.id == state.id)
                .ok_or(DiscoveryError::CursorExpired)?;
            let identity = provider
                .runtime
                .revalidate(admission)
                .await
                .map_err(|_| DiscoveryError::ProviderUnavailable)?;
            if !identity.supports_source_origin(origin.as_str()) {
                return Err(DiscoveryError::ProviderUnavailable);
            }
            if !state.listing_epoch.is_empty()
                && String::from(identity.listing_epoch) != state.listing_epoch
            {
                return Err(DiscoveryError::CursorExpired);
            }
        }
    }
    let count = federation.providers.len().max(1);
    let base = usize::from(request.limit) / count;
    let remainder = usize::from(request.limit) % count;
    let calls = federation
        .providers
        .iter()
        .enumerate()
        .filter_map(|(index, state)| {
            let quota = base + usize::from(index < remainder);
            let provider = selected
                .iter()
                .find(|provider| provider.view.id == state.id)?;
            let needed = quota.saturating_sub(state.page.items.len());
            (needed > 0 && matches!(state.page.outcome.as_str(), "participating" | "pending"))
                .then_some(async move {
                    let result = match provider.runtime.qualify(admission, false).await {
                        Ok(identity) if supports_origin(&identity, request.source_origin) => {
                            let limit =
                                provider_request_limit(needed, Some(identity.max_page_size));
                            let mut body = provider_list_body(
                                &request.query,
                                limit,
                                state.upstream_cursor.as_deref(),
                            );
                            set_origin(&mut body, request.source_origin);
                            provider
                                .runtime
                                .call(Operation::List, body, admission)
                                .await
                        }
                        Ok(_) => Err(ProviderError::Failed(Failure::Incompatible)),
                        Err(error) => Err(error),
                    };
                    (index, result)
                })
        });
    for (index, result) in join_all(calls).await {
        apply_reply(
            &mut federation.providers[index],
            result,
            request.source_origin,
        );
    }
    Ok(())
}

// A ranked merge cannot emit while a participating source has an unknown head.
async fn fetch_new_page(
    selected: &[super::manager::Provider],
    federation: &mut Federation,
    request: &DiscoveryRequest,
    admission: &super::scheduler::Admission,
) -> Result<Vec<Value>, DiscoveryError> {
    let as_of = federation
        .as_of
        .clone()
        .ok_or(DiscoveryError::CursorExpired)?;
    let mut items = Vec::new();
    // Buffered rows are not an authorization lease. Requalify every source on
    // each frontend page, including sources that do not need another fetch.
    for state in &mut federation.providers {
        let provider = selected
            .iter()
            .find(|provider| provider.view.id == state.id)
            .ok_or(DiscoveryError::CursorExpired)?;
        match provider.runtime.revalidate(admission).await {
            Ok(identity)
                if identity.supports_new_feed()
                    && supports_origin(&identity, request.source_origin)
                    && (state.listing_epoch.is_empty()
                        || String::from(identity.listing_epoch.clone()) == state.listing_epoch) =>
            {
                if state.page.outcome == "pending" && !state.page.items.is_empty() {
                    state.page.outcome = if state.upstream_cursor.is_some() {
                        "participating"
                    } else {
                        "exhausted"
                    }
                    .into();
                }
            }
            Err(ProviderError::Pending) if !federation.ranked_started => {
                state.page.outcome = "pending".into();
                return Ok(Vec::new());
            }
            _ if federation.ranked_started => return Err(DiscoveryError::CursorExpired),
            _ => return Err(DiscoveryError::ProviderUnavailable),
        }
    }
    // Each productive refill yields at least one item. This bounds malicious
    // empty continuations as well as network work independently of the deadline.
    for _ in 0..=usize::from(request.limit) {
        let calls = federation
            .providers
            .iter_mut()
            .filter(|state| {
                state.page.items.is_empty()
                    && matches!(state.page.outcome.as_str(), "participating" | "pending")
            })
            .map(|state| {
                let as_of = &as_of;
                async move {
                    let provider = selected
                        .iter()
                        .find(|provider| provider.view.id == state.id)
                        .ok_or(DiscoveryError::CursorExpired)?;
                    let reply = match provider.runtime.qualify(admission, false).await {
                        Ok(identity)
                            if identity.supports_new_feed()
                                && supports_origin(&identity, request.source_origin) =>
                        {
                            let batch = *state
                                .new_batch_size
                                .get_or_insert(request.limit.min(identity.max_page_size));
                            let mut body = provider_list_body(
                                &request.query,
                                usize::from(batch),
                                state.upstream_cursor.as_deref(),
                            );
                            body["feed"] = Value::String("new".into());
                            body["asOf"] = Value::String(as_of.clone());
                            set_origin(&mut body, request.source_origin);
                            provider
                                .runtime
                                .call(Operation::List, body, admission)
                                .await
                        }
                        Ok(_) => Err(ProviderError::Failed(Failure::Incompatible)),
                        Err(error) => Err(error),
                    };
                    if let Ok(reply) = &reply {
                        validate_origin_reply(reply, request.source_origin)?;
                        validate_new_reply(state, reply, as_of)?;
                    }
                    apply_reply(state, reply, request.source_origin);
                    Ok::<(), DiscoveryError>(())
                }
            });
        for result in join_all(calls).await {
            result?;
        }
        let pending = federation
            .providers
            .iter()
            .any(|state| state.page.outcome == "pending");
        let failed = federation
            .providers
            .iter()
            .any(|state| state.page.outcome == "failed");
        if failed && !federation.ranked_started {
            return Err(DiscoveryError::ProviderUnavailable);
        }
        if federation.ranked_started && (pending || failed) {
            return Err(DiscoveryError::CursorExpired);
        }
        if pending {
            // Nothing has been consumed; retry the chain before ranking begins.
            return Ok(Vec::new());
        }
        let winner = federation
            .providers
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                state
                    .page
                    .items
                    .front()
                    .map(|item| new_key(item).map(|key| (key, state.id.clone(), index)))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min();
        let Some((_, _, index)) = winner else {
            return Ok(items);
        };
        let state = &mut federation.providers[index];
        let raw = state
            .page
            .items
            .pop_front()
            .ok_or(DiscoveryError::InvalidProvider)?;
        items.push(project(&state.id, raw)?);
        federation.ranked_started = true;
        if items.len() == usize::from(request.limit) {
            return Ok(items);
        }
    }
    Err(DiscoveryError::Capacity)
}

fn new_key(raw: &Value) -> Result<(i64, String), DiscoveryError> {
    let timestamp = raw
        .get("firstSeenAt")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 64)
        .and_then(|value| value.parse::<jiff::Timestamp>().ok())
        .ok_or(DiscoveryError::InvalidProvider)?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .ok_or(DiscoveryError::InvalidProvider)?;
    Ok((-timestamp.as_microsecond(), id.to_owned()))
}

fn validate_new_reply(
    state: &mut FederatedProvider,
    reply: &super::provider::Reply,
    as_of: &str,
) -> Result<(), DiscoveryError> {
    let result = &reply.result;
    let cutoff = as_of
        .parse::<jiff::Timestamp>()
        .map_err(|_| DiscoveryError::InvalidProvider)?;
    let reported = result
        .get("asOf")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 64)
        .and_then(|value| value.parse::<jiff::Timestamp>().ok());
    if !reply.identity.supports_new_feed()
        || result["feed"] != "new"
        || result["rankingVersion"] != "new/v1"
        || reported != Some(cutoff)
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    let coverage = result
        .get("coverage")
        .ok_or(DiscoveryError::InvalidProvider)?;
    if !matches!(
        coverage["population"].as_str(),
        Some("hosted" | "hosted-and-skills")
    ) || coverage["complete"].as_bool().is_none()
        || !coverage["unknownFirstSeen"]
            .as_u64()
            .is_some_and(|count| count <= MAX_SAFE_INTEGER)
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    if state
        .new_coverage
        .as_ref()
        .is_some_and(|prior| prior != coverage)
    {
        return Err(DiscoveryError::CursorExpired);
    }
    let rows = result["artifacts"]
        .as_array()
        .ok_or(DiscoveryError::InvalidProvider)?;
    if rows.len()
        > usize::from(
            state
                .new_batch_size
                .ok_or(DiscoveryError::InvalidProvider)?,
        )
        || (rows.is_empty()
            && result
                .get("nextCursor")
                .is_some_and(|value| !value.is_null()))
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    let mut last = state.new_last_key.clone();
    for row in rows {
        let key = new_key(row)?;
        let instant = -key.0;
        if instant > cutoff.as_microsecond()
            || instant < cutoff.as_microsecond() - 604_800_000_000
            || last.as_ref().is_some_and(|prior| prior >= &key)
        {
            return Err(DiscoveryError::InvalidProvider);
        }
        last = Some(key);
    }
    state.new_last_key = last;
    state.new_coverage = Some(serde_json::json!({
        "population": coverage["population"], "complete": coverage["complete"],
        "unknownFirstSeen": coverage["unknownFirstSeen"]
    }));
    Ok(())
}

#[cfg(test)]
#[path = "discovery_workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
mod new_ranking_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn populated_ranked_response_is_not_reported_as_empty() {
        let mut provider = ProviderPage::pending("catalog");
        provider.outcome = "exhausted".into();
        provider.total = Some(1);
        let summary = merge_page(&mut [provider], 0, 10).unwrap();
        assert_eq!(summary.state, "empty");
        assert_eq!(
            with_ranked_items(summary.clone(), Vec::new()).state,
            "empty"
        );
        let response = with_ranked_items(summary, vec![json!({"artifactId":"new"})]);
        assert_eq!(response.state, "complete");
        assert_eq!(response.known_total, Some(1));
        assert!(response.coverage_complete);
    }

    fn state() -> FederatedProvider {
        FederatedProvider {
            id: "catalog".into(),
            incarnation: "boot".into(),
            listing_epoch: "listing".into(),
            max_page_size: Some(200),
            upstream_cursor: None,
            page: ProviderPage::pending("catalog"),
            new_batch_size: Some(2),
            new_last_key: None,
            new_coverage: None,
        }
    }

    fn reply(rows: Value) -> super::super::provider::Reply {
        super::super::provider::Reply {
            identity: super::super::provider::Identity::parse(json!({
                "contractVersion":"depot.discovery/v1", "deploymentId":"deployment",
                "deploymentEpoch":"boot", "authorityEpoch":"authority", "listingEpoch":"listing",
                "snapshotContinuations":true,"maxPageSize":200,
                "feeds":{"new":{"rankingVersion":"new/v1","windowSeconds":604800,"acceptsAsOf":true}}
            })).unwrap(),
            result: json!({"feed":"new","rankingVersion":"new/v1","asOf":"2026-09-08T12:00:00Z",
                "artifacts":rows,"coverage":{"population":"hosted","complete":false,"unknownFirstSeen":3}}),
        }
    }

    #[test]
    fn new_order_uses_instants_and_stable_identity_not_timestamp_strings() {
        let first = new_key(&json!({"id":"a","firstSeenAt":"2026-09-08T12:00:00Z"})).unwrap();
        let equal = new_key(&json!({"id":"b","firstSeenAt":"2026-09-08T07:00:00-05:00"})).unwrap();
        let older = new_key(&json!({"id":"a","firstSeenAt":"2026-09-08T11:59:59Z"})).unwrap();
        assert!(first < equal && equal < older);
        assert!(new_key(&json!({"id":"a"})).is_err());
    }

    #[test]
    fn new_reply_rejects_reverse_order_across_pages_and_window_mismatch() {
        let mut state = state();
        let cutoff = "2026-09-08T12:00:00Z";
        validate_new_reply(
            &mut state,
            &reply(json!([
                {"id":"a","firstSeenAt":"2026-09-08T11:00:00Z"},
                {"id":"b","firstSeenAt":"2026-09-08T10:00:00Z"}
            ])),
            cutoff,
        )
        .unwrap();
        assert!(
            validate_new_reply(
                &mut state,
                &reply(json!([
                    {"id":"c","firstSeenAt":"2026-09-08T10:30:00Z"}
                ])),
                cutoff
            )
            .is_err()
        );
        assert!(validate_new_reply(&mut state, &reply(json!([])), "2026-09-08T12:01:00Z").is_err());
    }

    #[test]
    fn new_reply_rejects_empty_continuation_and_excess_batch() {
        let mut empty = reply(json!([]));
        empty.result["nextCursor"] = json!("next");
        assert!(validate_new_reply(&mut state(), &empty, "2026-09-08T12:00:00Z").is_err());
        let mut small = state();
        small.new_batch_size = Some(1);
        assert!(
            validate_new_reply(
                &mut small,
                &reply(json!([
                    {"id":"a","firstSeenAt":"2026-09-08T11:00:00Z"},
                    {"id":"b","firstSeenAt":"2026-09-08T10:00:00Z"}
                ])),
                "2026-09-08T12:00:00Z"
            )
            .is_err()
        );
    }
}

fn supports_origin(identity: &super::provider::Identity, origin: Option<SourceOrigin>) -> bool {
    origin.is_none_or(|origin| identity.supports_source_origin(origin.as_str()))
}

fn set_origin(body: &mut Value, origin: Option<SourceOrigin>) {
    if let Some(origin) = origin {
        body["sourceOrigin"] = Value::String(origin.as_str().into());
    }
}

pub(super) fn validate_origin_reply(
    reply: &super::provider::Reply,
    origin: Option<SourceOrigin>,
) -> Result<(), DiscoveryError> {
    let Some(origin) = origin else {
        return Ok(());
    };
    if !supports_origin(&reply.identity, Some(origin))
        || reply.result["sourceOrigin"].as_str() != Some(origin.as_str())
        || !reply.result["artifacts"].as_array().is_some_and(|rows| {
            rows.iter()
                .all(|row| row["sourceOrigin"].as_str() == Some(origin.as_str()))
        })
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    Ok(())
}

pub(super) fn provider_list_body(query: &str, limit: usize, cursor: Option<&str>) -> Value {
    let mut body = serde_json::json!({"limit": limit});
    // Depot treats an omitted query as an unfiltered listing, but rejects "".
    if !query.trim().is_empty() {
        body["query"] = Value::String(query.to_owned());
    }
    if let Some(cursor) = cursor {
        body["cursor"] = Value::String(cursor.to_owned());
    }
    body
}

pub(super) fn provider_request_limit(requested: usize, advertised: Option<u16>) -> usize {
    requested.min(usize::from(advertised.unwrap_or(MAX_PAGE)))
}

fn apply_reply(
    state: &mut FederatedProvider,
    reply: Result<super::provider::Reply, ProviderError>,
    origin: Option<SourceOrigin>,
) {
    match reply {
        Ok(reply)
            if state.listing_epoch.is_empty()
                || String::from(reply.identity.listing_epoch.clone()) == state.listing_epoch =>
        {
            if validate_origin_reply(&reply, origin).is_err() {
                state.page = ProviderPage::failed(&state.id, "incompatible");
                return;
            }
            let Some(result) = reply.result.as_object() else {
                state.page = ProviderPage::failed(&state.id, "incompatible");
                return;
            };
            let Some(items) = result.get("artifacts").and_then(Value::as_array) else {
                state.page = ProviderPage::failed(&state.id, "incompatible");
                return;
            };
            state.listing_epoch = reply.identity.listing_epoch.into();
            state.max_page_size = Some(reply.identity.max_page_size);
            state.upstream_cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            state.page.items.extend(items.iter().cloned());
            state.page.total = result
                .get("total")
                .and_then(Value::as_u64)
                .filter(|total| *total <= MAX_SAFE_INTEGER);
            state.page.outcome = if state.upstream_cursor.is_none() {
                "exhausted"
            } else {
                "participating"
            }
            .into();
        }
        Ok(_) => state.page = ProviderPage::failed(&state.id, "catalog_changed"),
        Err(ProviderError::Pending) => state.page.outcome = "pending".into(),
        Err(error) => state.page = ProviderPage::failed(&state.id, failure_kind(error)),
    }
}

fn failure_kind(error: ProviderError) -> &'static str {
    match error {
        ProviderError::Pending => "pending",
        ProviderError::Stale => "catalog_changed",
        ProviderError::Disabled => "disabled",
        ProviderError::Failed(Failure::Unauthorized) => "unauthorized",
        ProviderError::Failed(Failure::Incompatible | Failure::NotFound) => "incompatible",
        ProviderError::Failed(Failure::SnapshotChanged) => "catalog_changed",
        ProviderError::Failed(Failure::Configuration) => "configuration",
        ProviderError::Failed(Failure::Transient) => "unavailable",
    }
}

impl From<CursorError> for DiscoveryError {
    fn from(error: CursorError) -> Self {
        match error {
            CursorError::Expired => Self::CursorExpired,
            CursorError::Capacity => Self::Capacity,
            CursorError::Invalid => Self::InvalidProvider,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Federation {
    start: usize,
    #[serde(default)]
    as_of: Option<String>,
    #[serde(default)]
    ranked_started: bool,
    providers: Vec<FederatedProvider>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FederatedProvider {
    id: String,
    incarnation: String,
    listing_epoch: String,
    #[serde(default)]
    max_page_size: Option<u16>,
    upstream_cursor: Option<String>,
    page: ProviderPage,
    #[serde(default)]
    new_batch_size: Option<u16>,
    #[serde(default)]
    new_last_key: Option<(i64, String)>,
    #[serde(default)]
    new_coverage: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPage {
    pub provider_id: String,
    pub outcome: String,
    pub items: VecDeque<Value>,
    pub next_cursor: Option<String>,
    pub total: Option<u64>,
    pub failure: Option<String>,
}

impl ProviderPage {
    pub fn participating(
        id: &str,
        items: Vec<Value>,
        next_cursor: Option<String>,
        total: Option<u64>,
    ) -> Self {
        Self {
            provider_id: id.into(),
            outcome: "participating".into(),
            items: items.into(),
            next_cursor,
            total,
            failure: None,
        }
    }
    pub fn pending(id: &str) -> Self {
        Self {
            provider_id: id.into(),
            outcome: "pending".into(),
            items: VecDeque::new(),
            next_cursor: None,
            total: None,
            failure: None,
        }
    }
    pub fn failed(id: &str, failure: &str) -> Self {
        Self {
            provider_id: id.into(),
            outcome: "failed".into(),
            items: VecDeque::new(),
            next_cursor: None,
            total: None,
            failure: Some(failure.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_origin: Option<SourceOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<DiscoveryFeed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_coverage: Option<Vec<Value>>,
    pub schema_version: String,
    pub scope: String,
    pub scope_epoch: String,
    pub items: Vec<Value>,
    pub provider_outcomes: Vec<ProviderOutcome>,
    pub failures: Vec<ProviderFailure>,
    pub coverage_complete: bool,
    pub known_total: Option<u64>,
    pub total_is_exact: bool,
    pub state: String,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOutcome {
    pub provider_id: String,
    pub state: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFailure {
    pub provider_id: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailResponse {
    pub schema_version: String,
    pub provider_id: String,
    pub artifact_id: String,
    pub artifact: Value,
}

pub async fn detail(
    manager: &Manager,
    authority: &labby_auth::browser_authority::BrowserAuthority,
    provider_id: &str,
    artifact_id: &str,
    receipt: tokio::time::Instant,
) -> Result<DetailResponse, DiscoveryError> {
    if provider_id.is_empty()
        || provider_id.len() > 64
        || artifact_id.is_empty()
        || artifact_id.len() > 512
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    let topology = manager.snapshot();
    let provider = topology
        .providers
        .get(provider_id)
        .filter(|provider| provider.view.enabled)
        .ok_or(DiscoveryError::ProviderUnavailable)?;
    let binding = Binding::for_browser(
        authority,
        "lab:read",
        provider_id.into(),
        artifact_id.into(),
        "detail/v1".into(),
        format!("selected:{provider_id}"),
        vec![(
            provider_id.into(),
            provider.runtime.incarnation().into(),
            String::new(),
        )],
    )
    .await?;
    let admission = manager
        .scheduler
        .admit(&binding.actor, receipt)
        .await
        .map_err(|_| DiscoveryError::Capacity)?;
    let reply = provider
        .runtime
        .call(
            Operation::Get,
            serde_json::json!({"artifactId": artifact_id}),
            &admission,
        )
        .await
        .map_err(|error| match error {
            ProviderError::Pending => DiscoveryError::Capacity,
            _ => DiscoveryError::ProviderUnavailable,
        })?;
    let raw = reply
        .result
        .get("artifact")
        .cloned()
        .ok_or(DiscoveryError::InvalidProvider)?;
    let artifact = project_detail(artifact_id, raw)?;
    Ok(DetailResponse {
        schema_version: "labby.depot-compatibility/v2".into(),
        provider_id: provider_id.into(),
        artifact_id: artifact_id.into(),
        artifact,
    })
}

pub fn validate_request(query: &str, limit: u16) -> Result<(), DiscoveryError> {
    let chars = query.chars().count();
    if chars > 200 || (!query.trim().is_empty() && chars < 3) {
        return Err(DiscoveryError::InvalidQuery);
    }
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(DiscoveryError::InvalidLimit);
    }
    Ok(())
}

pub fn merge_page(
    providers: &mut [ProviderPage],
    start: usize,
    limit: u16,
) -> Result<DiscoveryResponse, DiscoveryError> {
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(DiscoveryError::InvalidLimit);
    }
    providers.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
    let mut items = Vec::new();
    if !providers.is_empty() {
        let mut position = start % providers.len();
        let mut empty_pass = 0;
        while items.len() < limit as usize && empty_pass < providers.len() {
            if let Some(raw) = providers[position].items.pop_front() {
                items.push(project(&providers[position].provider_id, raw)?);
                empty_pass = 0;
            } else {
                empty_pass += 1;
            }
            position = (position + 1) % providers.len();
        }
    }
    let failures: Vec<_> = providers
        .iter()
        .filter_map(|provider| {
            provider.failure.as_ref().map(|kind| ProviderFailure {
                provider_id: provider.provider_id.clone(),
                kind: kind.clone(),
            })
        })
        .collect();
    let pending = providers
        .iter()
        .any(|provider| provider.outcome == "pending");
    let successful = providers
        .iter()
        .any(|provider| matches!(provider.outcome.as_str(), "participating" | "exhausted"));
    let coverage_complete = failures.is_empty() && !pending;
    let total_is_exact =
        coverage_complete && providers.iter().all(|provider| provider.total.is_some());
    let totals: Vec<_> = providers
        .iter()
        .filter_map(|provider| provider.total)
        .collect();
    let known_total = (!totals.is_empty())
        .then(|| {
            totals.into_iter().try_fold(0_u64, |sum, value| {
                sum.checked_add(value)
                    .filter(|sum| *sum <= MAX_SAFE_INTEGER)
            })
        })
        .flatten();
    let state = if !failures.is_empty() && successful {
        "partial"
    } else if pending {
        "deferred"
    } else if !failures.is_empty() {
        "all_failed"
    } else if providers.is_empty() {
        "all_disabled"
    } else if items.is_empty() {
        "empty"
    } else {
        "complete"
    }
    .to_owned();
    let response = DiscoveryResponse {
        source_origin: None,
        feed: None,
        as_of: None,
        ranking_version: None,
        feed_coverage: None,
        schema_version: "labby.depot-compatibility/v2".into(),
        scope: String::new(),
        scope_epoch: String::new(),
        items,
        provider_outcomes: providers
            .iter()
            .map(|provider| ProviderOutcome {
                provider_id: provider.provider_id.clone(),
                state: provider.outcome.clone(),
            })
            .collect(),
        failures,
        coverage_complete,
        known_total,
        total_is_exact,
        state,
        next_cursor: None,
    };
    if serde_json::to_vec(&response)
        .map_err(|_| DiscoveryError::InvalidProvider)?
        .len()
        > MAX_RESPONSE
    {
        return Err(DiscoveryError::ResponseTooLarge);
    }
    Ok(response)
}

fn project(provider: &str, raw: Value) -> Result<Value, DiscoveryError> {
    if provider.is_empty() || provider.len() > 64 {
        return Err(DiscoveryError::InvalidProvider);
    }
    let source = raw.as_object().ok_or(DiscoveryError::InvalidProvider)?;
    let id = source
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 512)
        .ok_or(DiscoveryError::InvalidProvider)?;
    let mut projected = Map::new();
    projected.insert("providerId".into(), Value::String(provider.into()));
    projected.insert("artifactId".into(), Value::String(id.into()));
    projected.extend(project_fields(source)?);
    Ok(Value::Object(projected))
}

pub(super) fn project_detail(expected_id: &str, raw: Value) -> Result<Value, DiscoveryError> {
    let source = raw.as_object().ok_or(DiscoveryError::InvalidProvider)?;
    let descriptor = source
        .get("descriptor")
        .and_then(Value::as_object)
        .ok_or(DiscoveryError::InvalidProvider)?;
    let actual = source
        .get("id")
        .or_else(|| descriptor.get("id"))
        .and_then(Value::as_str)
        .ok_or(DiscoveryError::InvalidProvider)?;
    if actual != expected_id
        || actual.is_empty()
        || actual.len() > 512
        || descriptor
            .get("id")
            .is_some_and(|value| value.as_str() != Some(actual))
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    let mut result = Map::new();
    result.insert("id".into(), Value::String(actual.into()));
    result.extend(project_fields(source)?);
    if let Some(readme) = source.get("readme") {
        result.insert("readme".into(), project_readme(source, readme)?);
    }
    if let Some(lineage) = source.get("lineage") {
        result.insert("lineage".into(), project_lineage(lineage)?);
    }
    let value = Value::Object(result);
    if serde_json::to_vec(&value)
        .map_err(|_| DiscoveryError::InvalidProvider)?
        .len()
        > MAX_RESPONSE
    {
        return Err(DiscoveryError::ResponseTooLarge);
    }
    Ok(value)
}

fn project_readme(source: &Map<String, Value>, value: &Value) -> Result<Value, DiscoveryError> {
    let object = value.as_object().ok_or(DiscoveryError::InvalidProvider)?;
    let valid = match object.get("state").and_then(Value::as_str) {
        Some("available") => {
            let revision = object.get("revisionId").and_then(Value::as_str);
            object.len() == 5
                && matches!(
                    (
                        object.get("kind").and_then(Value::as_str),
                        object.get("path").and_then(Value::as_str)
                    ),
                    (Some("readme"), Some("README.md")) | (Some("skill"), Some("SKILL.md"))
                )
                && object
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.len() <= 65_536 && !text.contains('\0'))
                && revision.is_some_and(|id| !id.is_empty() && id.len() <= 512)
                && source
                    .get("currentRevision")
                    .and_then(|revision| revision.get("id"))
                    .and_then(Value::as_str)
                    == revision
                && source
                    .get("currentRevisionId")
                    .is_none_or(|id| id.as_str() == revision)
        }
        Some("unavailable") => {
            object.len() == 2
                && matches!(
                    object.get("reason").and_then(Value::as_str),
                    Some(
                        "absent"
                            | "not_distributable"
                            | "too_large"
                            | "storage_unavailable"
                            | "invalid_text"
                    )
                )
        }
        _ => false,
    };
    if valid {
        Ok(value.clone())
    } else {
        Err(DiscoveryError::InvalidProvider)
    }
}

fn project_lineage(value: &Value) -> Result<Value, DiscoveryError> {
    let object = value.as_object().ok_or(DiscoveryError::InvalidProvider)?;
    const ARTIFACTS: [&str; 2] = ["upstreamArtifactId", "forkedFromArtifactId"];
    const REVISIONS: [&str; 3] = [
        "upstreamRevisionId",
        "forkedFromRevisionId",
        "lastObservedUpstreamRevisionId",
    ];
    let canonical = |id: &str| {
        !id.is_empty()
            && id.len() <= 160
            && id
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
    };
    let revision = |id: &str| {
        canonical(id)
            || id.strip_prefix("sha256:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
    };
    let valid_nullable = |field: &str, validate: &dyn Fn(&str) -> bool| {
        object
            .get(field)
            .is_some_and(|value| value.is_null() || value.as_str().is_some_and(validate))
    };
    if object.len() != 6
        || object.get("following").and_then(Value::as_bool).is_none()
        || !ARTIFACTS
            .iter()
            .all(|field| valid_nullable(field, &canonical))
        || !REVISIONS
            .iter()
            .all(|field| valid_nullable(field, &revision))
        || (object["upstreamArtifactId"].is_null()
            && (!object["upstreamRevisionId"].is_null()
                || !object["lastObservedUpstreamRevisionId"].is_null()))
        || (object["forkedFromArtifactId"].is_null() && !object["forkedFromRevisionId"].is_null())
    {
        return Err(DiscoveryError::InvalidProvider);
    }
    Ok(value.clone())
}

const DESCRIPTOR_TEXT: &[(&str, usize)] = &[
    ("id", 512),
    ("kind", 128),
    ("namespace", 512),
    ("name", 512),
    ("title", 4096),
    ("description", MAX_FIELD),
];

fn project_text_fields(
    source: &Map<String, Value>,
    fields: &[(&str, usize)],
) -> Result<Map<String, Value>, DiscoveryError> {
    let mut projected = Map::new();
    for &(field, limit) in fields {
        let Some(value) = source.get(field) else {
            continue;
        };
        // These optional display fields are nullable in Depot's real catalog.
        if value.is_null() && matches!(field, "title" | "description") {
            continue;
        }
        let text = value.as_str().ok_or(DiscoveryError::InvalidProvider)?;
        if text.len() > limit || (field == "id" && text.is_empty()) {
            return Err(DiscoveryError::InvalidProvider);
        }
        projected.insert(field.into(), value.clone());
    }
    Ok(projected)
}

fn project_fields(source: &Map<String, Value>) -> Result<Map<String, Value>, DiscoveryError> {
    let mut projected = project_text_fields(source, DESCRIPTOR_TEXT)?;
    if let Some(origin) = source.get("sourceOrigin") {
        if !origin.is_null()
            && !matches!(
                origin.as_str(),
                Some("mcp-registry" | "acp-registry" | "ard")
            )
        {
            return Err(DiscoveryError::InvalidProvider);
        }
        projected.insert("sourceOrigin".into(), origin.clone());
    }
    if let Some(value) = source.get("provenance") {
        let provenance = value.as_object().ok_or(DiscoveryError::InvalidProvider)?;
        let mut format = Map::new();
        for field in ["originalFormat", "originalVersion"] {
            if let Some(value) = provenance.get(field) {
                if !value.is_null()
                    && !value
                        .as_str()
                        .is_some_and(|text| text.len() <= 128 && !text.contains('\0'))
                {
                    return Err(DiscoveryError::InvalidProvider);
                }
                format.insert(field.into(), value.clone());
            }
        }
        projected.insert("provenance".into(), Value::Object(format));
    }
    project_timestamps(source, &mut projected)?;
    projected.extend(project_text_fields(
        source,
        &[("currentRevisionId", 512), ("contentDigest", 512)],
    )?);
    for (field, fields) in [
        ("descriptor", DESCRIPTOR_TEXT),
        (
            "currentRevision",
            &[("id", 512), ("contentDigest", 512)][..],
        ),
        (
            "license",
            &[
                ("redistribution", 128),
                ("reviewState", 128),
                ("takedownState", 128),
            ][..],
        ),
        (
            "publication",
            &[("state", 128), ("visibility", 128), ("distribution", 128)][..],
        ),
    ] {
        if let Some(value) = source.get(field) {
            if !bounded_value(value, 0) {
                return Err(DiscoveryError::InvalidProvider);
            }
            let nested = value.as_object().ok_or(DiscoveryError::InvalidProvider)?;
            let mut summary = project_text_fields(nested, fields)?;
            if field == "descriptor"
                && let Some(tags) = nested.get("tags")
            {
                let values = tags.as_array().ok_or(DiscoveryError::InvalidProvider)?;
                let mut seen = std::collections::HashSet::new();
                if values.len() > 64
                    || values.iter().any(|value| {
                        !value.as_str().is_some_and(|tag| {
                            !tag.is_empty()
                                && tag.len() <= 64
                                && !tag.contains('\0')
                                && seen.insert(tag)
                        })
                    })
                {
                    return Err(DiscoveryError::InvalidProvider);
                }
                summary.insert("tags".into(), tags.clone());
            }
            if field == "currentRevision" {
                project_timestamp(nested, &mut summary, "authoredAt")?;
                if let Some(count) = nested.get("fileCount") {
                    // Depot revisions contain at most 2,000 components; files are a subset.
                    if count.as_u64().is_none_or(|count| count > 2_000) {
                        return Err(DiscoveryError::InvalidProvider);
                    }
                    summary.insert("fileCount".into(), count.clone());
                }
            }
            if field == "license"
                && let Some(declared) = nested.get("declared")
            {
                if !declared.is_null()
                    && !declared.as_str().is_some_and(|value| value.len() <= 1024)
                {
                    return Err(DiscoveryError::InvalidProvider);
                }
                summary.insert("declared".into(), declared.clone());
            }
            projected.insert(field.into(), Value::Object(summary));
        }
    }
    if let Some(count) = source.get("revisionCount") {
        if count.as_u64().is_none_or(|count| count > MAX_SAFE_INTEGER) {
            return Err(DiscoveryError::InvalidProvider);
        }
        projected.insert("revisionCount".into(), count.clone());
    }
    Ok(projected)
}

fn project_timestamps(
    source: &Map<String, Value>,
    result: &mut Map<String, Value>,
) -> Result<(), DiscoveryError> {
    for field in ["createdAt", "updatedAt", "firstSeenAt"] {
        project_timestamp(source, result, field)?;
    }
    Ok(())
}

fn project_timestamp(
    source: &Map<String, Value>,
    result: &mut Map<String, Value>,
    field: &str,
) -> Result<(), DiscoveryError> {
    if let Some(value) = source.get(field) {
        if !bounded_timestamp(value) || (field == "firstSeenAt" && value.is_null()) {
            return Err(DiscoveryError::InvalidProvider);
        }
        result.insert(field.into(), value.clone());
    }
    Ok(())
}

fn bounded_timestamp(value: &Value) -> bool {
    value.is_null()
        || value
            .as_str()
            .is_some_and(|value| value.len() <= 64 && value.parse::<jiff::Timestamp>().is_ok())
}
fn bounded_value(value: &Value, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => value.len() <= MAX_FIELD,
        Value::Array(values) => {
            values.len() <= 64 && values.iter().all(|value| bounded_value(value, depth + 1))
        }
        Value::Object(values) => {
            values.len() <= 64
                && values
                    .iter()
                    .all(|(key, value)| key.len() <= 128 && bounded_value(value, depth + 1))
        }
    }
}
