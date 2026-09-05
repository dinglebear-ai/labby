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
    let current_binding = Binding::for_browser(
        authority,
        "lab:read",
        scope.clone(),
        request.query.clone(),
        format!("discovery/v1:{}", request.limit),
        registry_epoch.clone(),
        current,
    )
    .await?;
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
        let binding = Binding::for_browser(
            authority,
            "lab:read",
            scope,
            request.query.clone(),
            format!("discovery/v1:{}", request.limit),
            registry_epoch,
            providers,
        )
        .await?;
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
    fetch_pages(&selected, &mut federation, request, &admission).await;
    let mut pages: Vec<_> = federation
        .providers
        .iter()
        .map(|provider| provider.page.clone())
        .collect();
    let mut response = merge_page(&mut pages, federation.start, request.limit)?;
    response.scope = stored_binding.scope.clone();
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
    let page = lease.complete(bytes, state, now).await?;
    response.next_cursor = page.next_cursor;
    Ok(response)
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
            upstream_cursor: None,
            page: ProviderPage::participating(&provider.view.id, Vec::new(), None, None),
        },
        Err(ProviderError::Pending) => FederatedProvider {
            id: provider.view.id.clone(),
            incarnation: provider.runtime.incarnation().to_owned(),
            listing_epoch: String::new(),
            upstream_cursor: None,
            page: ProviderPage::pending(&provider.view.id),
        },
        Err(error) => FederatedProvider {
            id: provider.view.id.clone(),
            incarnation: provider.runtime.incarnation().to_owned(),
            listing_epoch: String::new(),
            upstream_cursor: None,
            page: ProviderPage::failed(&provider.view.id, failure_kind(error)),
        },
    }
}

async fn fetch_pages(
    selected: &[super::manager::Provider],
    federation: &mut Federation,
    request: &DiscoveryRequest,
    admission: &super::scheduler::Admission,
) {
    let count = federation.providers.len().max(1);
    let base = usize::from(request.limit) / count;
    let remainder = usize::from(request.limit) % count;
    let calls = federation.providers.iter().enumerate().filter_map(|(index, state)| {
        let quota = base + usize::from(index < remainder);
        let provider = selected.iter().find(|provider| provider.view.id == state.id)?;
        (quota > state.page.items.len() && matches!(state.page.outcome.as_str(), "participating" | "pending")).then_some(async move {
            let body = serde_json::json!({ "query": request.query, "limit": quota, "cursor": state.upstream_cursor });
            (index, provider.runtime.call(Operation::List, body, admission).await)
        })
    });
    for (index, result) in join_all(calls).await {
        apply_reply(&mut federation.providers[index], result);
    }
}

fn apply_reply(
    state: &mut FederatedProvider,
    reply: Result<super::provider::Reply, ProviderError>,
) {
    match reply {
        Ok(reply)
            if state.listing_epoch.is_empty()
                || String::from(reply.identity.listing_epoch.clone()) == state.listing_epoch =>
        {
            let Some(result) = reply.result.as_object() else {
                state.page = ProviderPage::failed(&state.id, "incompatible");
                return;
            };
            let Some(items) = result.get("artifacts").and_then(Value::as_array) else {
                state.page = ProviderPage::failed(&state.id, "incompatible");
                return;
            };
            state.listing_epoch = reply.identity.listing_epoch.into();
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
    providers: Vec<FederatedProvider>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FederatedProvider {
    id: String,
    incarnation: String,
    listing_epoch: String,
    upstream_cursor: Option<String>,
    page: ProviderPage,
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
    if chars > 200 || (chars != 0 && chars < 3) {
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
    for field in [
        "id",
        "kind",
        "namespace",
        "name",
        "title",
        "description",
        "currentRevisionId",
        "contentDigest",
        "license",
        "publication",
    ] {
        if let Some(value) = source.get(field) {
            if !bounded_value(value, 0) {
                return Err(DiscoveryError::InvalidProvider);
            }
            projected.insert(field.into(), value.clone());
        }
    }
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
    if actual != expected_id {
        return Err(DiscoveryError::InvalidProvider);
    }
    let mut result = Map::new();
    result.insert("id".into(), Value::String(actual.into()));
    for field in [
        "descriptor",
        "currentRevisionId",
        "currentRevision",
        "publication",
        "license",
    ] {
        if let Some(value) = source.get(field) {
            if !bounded_value(value, 0) {
                return Err(DiscoveryError::InvalidProvider);
            }
            result.insert(field.into(), value.clone());
        }
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
