//! Bounded operator notifications and Depot ingestion failure monitoring.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::dispatch::depot::DepotClient;
use crate::installation::InstallationPaths;

const DEFAULT_RETENTION: usize = 200;
const MAX_RETENTION: usize = 2_000;
const DEFAULT_DEPOT_MONITOR_INTERVAL_SECONDS: u64 = 30;
const MIN_DEPOT_MONITOR_INTERVAL_SECONDS: u64 = 10;
const MAX_DEPOT_MONITOR_INTERVAL_SECONDS: u64 = 3_600;
const MAX_NOTIFICATION_TEXT_CHARS: usize = 2_000;
const MAX_APPRISE_ATTEMPTS_PER_POLL: usize = 4;
const DEPOT_MONITOR_ACTOR: &str = "labby-depot-ingest-monitor";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: String,
    pub created_at_unix_ms: u64,
    pub level: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub dedupe_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct SourceCursor {
    key: String,
    at: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct NotificationState {
    records: Vec<NotificationRecord>,
    #[serde(default)]
    source_cursors: BTreeMap<String, SourceCursor>,
    #[serde(default)]
    pending_apprise: Vec<NotificationRecord>,
}

#[derive(Clone)]
pub struct NotificationCenter {
    state: Arc<RwLock<NotificationState>>,
    path: Option<Arc<PathBuf>>,
    retention: usize,
}

impl Default for NotificationCenter {
    fn default() -> Self {
        Self::memory(DEFAULT_RETENTION)
    }
}

impl NotificationCenter {
    #[must_use]
    pub fn memory(retention: usize) -> Self {
        Self {
            state: Arc::new(RwLock::new(NotificationState::default())),
            path: None,
            retention: retention.clamp(1, MAX_RETENTION),
        }
    }

    pub async fn open() -> Result<Self> {
        let retention =
            env_usize("LABBY_NOTIFICATION_RETENTION", DEFAULT_RETENTION).clamp(10, MAX_RETENTION);
        let path = InstallationPaths::resolve()?
            .root()
            .join("notifications.json");
        let state = load_state(&path, retention).await?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            path: Some(Arc::new(path)),
            retention,
        })
    }

    pub async fn list(&self) -> Vec<NotificationRecord> {
        self.state.read().await.records.clone()
    }

    pub async fn push_if_new(&self, record: NotificationRecord) -> Result<bool> {
        self.push_if_new_with_apprise(record, false).await
    }

    async fn push_if_new_with_apprise(
        &self,
        record: NotificationRecord,
        apprise_enabled: bool,
    ) -> Result<bool> {
        let mut state = self.state.write().await;
        if state
            .records
            .iter()
            .any(|existing| existing.dedupe_key == record.dedupe_key)
        {
            return Ok(false);
        }
        let mut snapshot = state.clone();
        if apprise_enabled {
            anyhow::ensure!(
                snapshot.pending_apprise.len() < MAX_RETENTION,
                "Apprise retry queue is full"
            );
            snapshot.pending_apprise.push(record.clone());
        }
        snapshot.records.insert(0, record);
        snapshot.records.truncate(self.retention);
        self.persist(&snapshot).await?;
        *state = snapshot;
        Ok(true)
    }

    async fn source_cursor(&self, source_id: &str) -> Option<SourceCursor> {
        self.state
            .read()
            .await
            .source_cursors
            .get(source_id)
            .cloned()
    }

    async fn advance_source_cursor(&self, source_id: &str, cursor: SourceCursor) -> Result<()> {
        let mut state = self.state.write().await;
        let mut snapshot = state.clone();
        snapshot.source_cursors.insert(source_id.to_owned(), cursor);
        self.persist(&snapshot).await?;
        *state = snapshot;
        Ok(())
    }

    async fn pending_apprise(&self) -> Vec<NotificationRecord> {
        self.state.read().await.pending_apprise.clone()
    }

    async fn mark_apprise_delivered(&self, key: &str) -> Result<()> {
        let mut state = self.state.write().await;
        let mut snapshot = state.clone();
        snapshot
            .pending_apprise
            .retain(|record| record.dedupe_key != key);
        self.persist(&snapshot).await?;
        *state = snapshot;
        Ok(())
    }

    async fn persist(&self, state: &NotificationState) -> Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        let body = serde_json::to_vec_pretty(state).context("serialize notifications")?;
        let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        tokio::fs::write(&temp, body)
            .await
            .with_context(|| format!("write {}", temp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("chmod {}", temp.display()))?;
        }
        tokio::fs::rename(&temp, path)
            .await
            .with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }
}

async fn load_state(path: &Path, retention: usize) -> Result<NotificationState> {
    let raw = match tokio::fs::read(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(NotificationState::default());
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut state: NotificationState = if raw.first() == Some(&b'[') {
        NotificationState {
            records: serde_json::from_slice(&raw).context("parse legacy notifications")?,
            ..Default::default()
        }
    } else {
        serde_json::from_slice(&raw).context("parse notifications")?
    };
    state.records.truncate(retention);
    Ok(state)
}

#[must_use]
pub(crate) fn notifications_enabled() -> bool {
    std::env::var("LABBY_NOTIFICATIONS_ENABLED")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

#[must_use]
pub(crate) fn depot_monitor_interval() -> Duration {
    Duration::from_secs(
        env_u64(
            "LABBY_DEPOT_MONITOR_INTERVAL_SECONDS",
            DEFAULT_DEPOT_MONITOR_INTERVAL_SECONDS,
        )
        .clamp(
            MIN_DEPOT_MONITOR_INTERVAL_SECONDS,
            MAX_DEPOT_MONITOR_INTERVAL_SECONDS,
        ),
    )
}

pub(crate) fn spawn_depot_failure_monitor(
    depot: Arc<DepotClient>,
    center: Arc<NotificationCenter>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !notifications_enabled() || !depot.status().configured {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(depot_monitor_interval());
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = poll_depot_failures(&depot, &center).await {
                tracing::warn!(
                    subsystem = "notifications",
                    source = "depot",
                    error = %error,
                    "Depot ingestion failure monitor poll failed"
                );
            }
        }
    }))
}

async fn poll_depot_failures(
    depot: &DepotClient,
    center: &NotificationCenter,
) -> std::result::Result<(), String> {
    let apprise = AppriseTarget::from_env();
    let mut attempted_apprise = BTreeSet::new();
    if let Some(target) = &apprise {
        deliver_pending_apprise(center, target, &mut attempted_apprise).await?;
    }
    depot
        .operations(DEPOT_MONITOR_ACTOR)
        .await
        .map_err(|error| format!("catalog: {error:?}"))?;
    let policy = depot
        .operation_policy("depot.sources.list", DEPOT_MONITOR_ACTOR)
        .await
        .map_err(|error| format!("policy: {error:?}"))?;
    let envelope = depot
        .call(
            "depot.sources.list",
            json!({}),
            DEPOT_MONITOR_ACTOR,
            policy,
            None,
        )
        .await
        .map_err(|error| format!("sources: {error:?}"))?;
    let sources = envelope
        .get("result")
        .and_then(|result| result.get("sources"))
        .and_then(Value::as_array)
        .ok_or_else(|| "sources response missing result.sources".to_string())?;

    for source in sources {
        let source_id = source
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let label = source_label(source);
        let Some(history) = source.get("history").and_then(Value::as_array) else {
            continue;
        };
        let newest_cursor = history.first().map(|event| SourceCursor {
            key: history_key(source_id, event),
            at: event
                .get("at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
        let previous_cursor = center.source_cursor(source_id).await;
        let unseen = unseen_history(source_id, history, previous_cursor.as_ref());
        for event in unseen.iter().rev() {
            if event.get("status").and_then(Value::as_str) != Some("failed") {
                continue;
            }
            let phase = event
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("refresh");
            let error = event
                .get("error")
                .and_then(Value::as_str)
                .map(bounded_text)
                .unwrap_or_else(|| "unknown ingestion failure".to_string());
            let dedupe_key = history_key(source_id, event);
            let record = NotificationRecord {
                id: uuid::Uuid::new_v4().to_string(),
                created_at_unix_ms: unix_millis(),
                level: "error".to_string(),
                title: "Depot ingestion failed".to_string(),
                body: bounded_text(&format!("{label} · {phase}: {error}")),
                source: "depot_ingest".to_string(),
                dedupe_key,
            };
            if center
                .push_if_new_with_apprise(record.clone(), apprise.is_some())
                .await
                .map_err(|error| format!("notification store: {error:#}"))?
            {
                tracing::error!(
                    subsystem = "notifications",
                    source = "depot_ingest",
                    source_id,
                    phase,
                    "Depot ingestion failure recorded"
                );
            }
        }
        if let Some(cursor) = newest_cursor {
            center
                .advance_source_cursor(source_id, cursor)
                .await
                .map_err(|error| format!("notification cursor: {error:#}"))?;
        }
    }
    if let Some(target) = &apprise {
        deliver_pending_apprise(center, target, &mut attempted_apprise).await?;
    }
    Ok(())
}

async fn deliver_pending_apprise(
    center: &NotificationCenter,
    target: &AppriseTarget,
    attempted: &mut BTreeSet<String>,
) -> std::result::Result<(), String> {
    for record in center.pending_apprise().await {
        if attempted.len() >= MAX_APPRISE_ATTEMPTS_PER_POLL {
            break;
        }
        if !attempted.insert(record.dedupe_key.clone()) {
            continue;
        }
        match target.send(&record).await {
            Ok(()) => center
                .mark_apprise_delivered(&record.dedupe_key)
                .await
                .map_err(|error| format!("notification delivery state: {error:#}"))?,
            Err(error) => {
                tracing::warn!(subsystem = "notifications", source = "apprise", error = %error,
                "Apprise notification delivery failed; will retry")
            }
        }
    }
    Ok(())
}

fn history_key(source_id: &str, event: &Value) -> String {
    let at = event.get("at").and_then(Value::as_str).unwrap_or("unknown");
    let job_id = event.get("jobId").and_then(Value::as_str).unwrap_or("");
    let phase = event
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or("refresh");
    let status = event
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!("depot:{source_id}:{at}:{job_id}:{phase}:{status}")
}

fn unseen_history<'a>(
    source_id: &str,
    history: &'a [Value],
    cursor: Option<&SourceCursor>,
) -> Vec<&'a Value> {
    let Some(cursor) = cursor else {
        return history.iter().collect();
    };
    if let Some(index) = history
        .iter()
        .position(|event| history_key(source_id, event) == cursor.key)
    {
        return history[..index].iter().collect();
    }
    // Depot retains only its latest 100 events. A parsed timestamp high-water
    // mark handles offsets and continues after that window moves. Equality is
    // included because Depot timestamps are second-granularity.
    tracing::warn!(
        subsystem = "notifications",
        source_id,
        "Depot history cursor fell out of the retained window; resuming by timestamp"
    );
    let previous_time = cursor.at.parse::<jiff::Timestamp>().ok();
    history
        .iter()
        .filter(|event| {
            let event_time = event
                .get("at")
                .and_then(Value::as_str)
                .and_then(|at| at.parse::<jiff::Timestamp>().ok());
            match (event_time, previous_time) {
                (Some(event_time), Some(previous_time)) => event_time >= previous_time,
                _ => true,
            }
        })
        .collect()
}

fn source_label(source: &Value) -> String {
    let args = source.get("args").and_then(Value::as_object);
    let target = args.and_then(|args| {
        ["url", "source", "endpoint", "domain"]
            .iter()
            .find_map(|key| args.get(*key).and_then(Value::as_str))
    });
    target
        .unwrap_or_else(|| {
            source
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("Depot source")
        })
        .to_string()
}

#[derive(Clone)]
struct AppriseTarget {
    endpoint: Url,
    client: reqwest::Client,
}

impl AppriseTarget {
    fn from_env() -> Option<Self> {
        let raw = std::env::var("APPRISE_URL").ok()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let mut endpoint = Url::parse(raw).ok()?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            tracing::warn!(
                subsystem = "notifications",
                source = "apprise",
                "APPRISE_URL is not a supported HTTP(S) base URL"
            );
            return None;
        }
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        {
            let mut segments = endpoint.path_segments_mut().ok()?;
            segments.pop_if_empty();
            segments.push("notify");
            if let Ok(key) = std::env::var("APPRISE_TOKEN") {
                let key = key.trim();
                if !key.is_empty() {
                    segments.push(key);
                }
            }
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self { endpoint, client })
    }

    async fn send(&self, record: &NotificationRecord) -> Result<()> {
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&json!({
                "title": record.title,
                "body": record.body,
                "type": if record.level == "error" { "failure" } else { "info" },
                "format": "text"
            }))
            .send()
            .await
            // reqwest's Display includes the full request URL, which contains
            // APPRISE_TOKEN in its path. Never propagate that error to logs.
            .map_err(|error| {
                anyhow::anyhow!(
                    "Apprise request failed ({})",
                    if error.is_timeout() {
                        "timeout"
                    } else if error.is_connect() {
                        "connect"
                    } else {
                        "transport"
                    }
                )
            })?;
        anyhow::ensure!(
            response.status().is_success(),
            "Apprise returned HTTP {}",
            response.status()
        );
        Ok(())
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn bounded_text(value: &str) -> String {
    value.chars().take(MAX_NOTIFICATION_TEXT_CHARS).collect()
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn apprise_client() -> reqwest::Client {
        drop(rustls::crypto::ring::default_provider().install_default());
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn bounded_center_deduplicates_records() {
        let center = NotificationCenter::memory(2);
        let record = NotificationRecord {
            id: "one".into(),
            created_at_unix_ms: 1,
            level: "error".into(),
            title: "failed".into(),
            body: "body".into(),
            source: "depot_ingest".into(),
            dedupe_key: "same".into(),
        };
        assert!(center.push_if_new(record.clone()).await.unwrap());
        assert!(!center.push_if_new(record).await.unwrap());
        assert_eq!(center.list().await.len(), 1);
    }

    #[tokio::test]
    async fn failed_persist_does_not_suppress_notification_retry() {
        let mut center = NotificationCenter::memory(2);
        let missing_dir =
            std::env::temp_dir().join(format!("labby-notifications-{}", uuid::Uuid::new_v4()));
        center.path = Some(Arc::new(missing_dir.join("notifications.json")));
        let record = NotificationRecord {
            id: "retry".into(),
            created_at_unix_ms: 1,
            level: "error".into(),
            title: "failed".into(),
            body: "body".into(),
            source: "depot_ingest".into(),
            dedupe_key: "retry".into(),
        };
        assert!(center.push_if_new(record.clone()).await.is_err());
        assert!(center.list().await.is_empty());
        tokio::fs::create_dir(&missing_dir).await.unwrap();
        assert!(center.push_if_new(record).await.unwrap());
        assert_eq!(center.list().await.len(), 1);
        tokio::fs::remove_dir_all(missing_dir).await.unwrap();
    }

    #[tokio::test]
    async fn legacy_notification_array_loads_and_migrates_on_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("notifications.json");
        let old = NotificationRecord {
            id: "old".into(),
            created_at_unix_ms: 1,
            level: "error".into(),
            title: "old".into(),
            body: "body".into(),
            source: "depot_ingest".into(),
            dedupe_key: "old".into(),
        };
        tokio::fs::write(&store_path, serde_json::to_vec(&vec![old.clone()]).unwrap())
            .await
            .unwrap();
        let center = NotificationCenter {
            state: Arc::new(RwLock::new(load_state(&store_path, 2).await.unwrap())),
            path: Some(Arc::new(store_path.clone())),
            retention: 2,
        };
        assert_eq!(center.list().await, vec![old]);
        center
            .advance_source_cursor(
                "source",
                SourceCursor {
                    key: "latest".into(),
                    at: "2026-09-29T00:00:00Z".into(),
                },
            )
            .await
            .unwrap();
        let migrated: Value =
            serde_json::from_slice(&tokio::fs::read(&store_path).await.unwrap()).unwrap();
        assert!(migrated["records"].is_array());
        assert_eq!(migrated["source_cursors"]["source"]["key"], "latest");
    }

    #[tokio::test]
    async fn apprise_failure_is_redacted_and_retried_after_store_reopen() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/notify/super-secret-key"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let target = AppriseTarget {
            endpoint: Url::parse(&format!("{}/notify/super-secret-key", server.uri())).unwrap(),
            client: apprise_client(),
        };
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("notifications.json");
        let center = NotificationCenter {
            state: Arc::new(RwLock::new(NotificationState::default())),
            path: Some(Arc::new(store_path.clone())),
            retention: 1,
        };
        let record = NotificationRecord {
            id: "retry".into(),
            created_at_unix_ms: 1,
            level: "error".into(),
            title: "failure".into(),
            body: "body".into(),
            source: "depot_ingest".into(),
            dedupe_key: "retry".into(),
        };
        assert!(
            center
                .push_if_new_with_apprise(record.clone(), true)
                .await
                .unwrap()
        );
        let disconnected = AppriseTarget {
            endpoint: Url::parse("http://127.0.0.1:1/notify/super-secret-key").unwrap(),
            client: apprise_client(),
        };
        let error = disconnected.send(&record).await.unwrap_err();
        assert!(!format!("{error:#}").contains("super-secret-key"));
        assert!(target.send(&record).await.is_err());
        // Evict the inbox record: delivery still waits in a separate durable queue.
        let mut next = record.clone();
        next.dedupe_key = "next".into();
        assert!(center.push_if_new(next).await.unwrap());
        let reopened = NotificationCenter {
            state: Arc::new(RwLock::new(load_state(&store_path, 1).await.unwrap())),
            path: Some(Arc::new(store_path)),
            retention: 1,
        };
        assert_eq!(reopened.pending_apprise().await.len(), 1);
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/notify/super-secret-key"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        for pending in reopened.pending_apprise().await {
            target.send(&pending).await.unwrap();
            reopened
                .mark_apprise_delivered(&pending.dedupe_key)
                .await
                .unwrap();
        }
        assert!(reopened.pending_apprise().await.is_empty());
    }

    #[tokio::test]
    async fn failed_apprise_retries_are_bounded_once_per_poll() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let target = AppriseTarget {
            endpoint: Url::parse(&format!("{}/notify/key", server.uri())).unwrap(),
            client: apprise_client(),
        };
        let center = NotificationCenter::memory(1);
        for index in 0..8 {
            let record = NotificationRecord {
                id: index.to_string(),
                created_at_unix_ms: 1,
                level: "error".into(),
                title: "failure".into(),
                body: "body".into(),
                source: "depot_ingest".into(),
                dedupe_key: index.to_string(),
            };
            assert!(center.push_if_new_with_apprise(record, true).await.unwrap());
        }
        let mut attempted = BTreeSet::new();
        deliver_pending_apprise(&center, &target, &mut attempted)
            .await
            .unwrap();
        deliver_pending_apprise(&center, &target, &mut attempted)
            .await
            .unwrap();
        assert_eq!(attempted.len(), MAX_APPRISE_ATTEMPTS_PER_POLL);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            MAX_APPRISE_ATTEMPTS_PER_POLL
        );
        assert_eq!(center.pending_apprise().await.len(), 8);
    }

    #[tokio::test]
    async fn cursor_prevents_replaying_evicted_failures_and_recovers_after_history_wrap() {
        let center = NotificationCenter::memory(1);
        let old = json!({"at":"2026-09-29T00:00:00Z", "status":"failed", "phase":"refresh"});
        let next = json!({"at":"2026-09-29T00:00:01Z", "status":"failed", "phase":"refresh"});
        let cursor = SourceCursor {
            key: history_key("source", &old),
            at: old["at"].as_str().unwrap().into(),
        };
        center
            .advance_source_cursor("source", cursor.clone())
            .await
            .unwrap();
        assert_eq!(
            unseen_history(
                "source",
                &[next.clone(), old.clone()],
                center.source_cursor("source").await.as_ref()
            )
            .len(),
            1
        );
        // More than Depot's 100-event window arrives between polls. The exact
        // cursor is gone, but every retained newer failure remains visible.
        let wrapped: Vec<Value> = (1..=100)
            .rev()
            .map(|second| json!({"at": format!("2026-09-29T00:{:02}:{:02}Z", second / 60, second % 60), "status":"failed"}))
            .collect();
        assert_eq!(unseen_history("source", &wrapped, Some(&cursor)).len(), 100);
        let same_second: Vec<Value> = (1..=100).rev()
            .map(|job| json!({"at":"2026-09-29T00:00:00Z", "status":"failed", "jobId":job.to_string()}))
            .collect();
        assert_eq!(
            unseen_history("source", &same_second, Some(&cursor)).len(),
            100
        );
        let offsets = [
            json!({"at":"2026-09-28T19:00:01-05:00", "status":"failed"}),
            json!({"at":"2026-09-28T18:59:59-05:00", "status":"failed"}),
        ];
        assert_eq!(unseen_history("source", &offsets, Some(&cursor)).len(), 1);
        let after = SourceCursor {
            key: history_key("source", &wrapped[0]),
            at: wrapped[0]["at"].as_str().unwrap().into(),
        };
        center.advance_source_cursor("source", after).await.unwrap();
        assert!(
            unseen_history(
                "source",
                &wrapped,
                center.source_cursor("source").await.as_ref()
            )
            .is_empty()
        );
    }

    #[test]
    fn source_label_prefers_repository_url() {
        assert_eq!(
            source_label(&json!({
                "id":"src_1",
                "args":{"url":"https://github.com/dinglebear-ai/labby"}
            })),
            "https://github.com/dinglebear-ai/labby"
        );
    }
}
