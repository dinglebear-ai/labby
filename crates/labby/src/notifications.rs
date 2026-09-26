//! Bounded operator notifications and Depot ingestion failure monitoring.

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

#[derive(Clone)]
pub struct NotificationCenter {
    records: Arc<RwLock<Vec<NotificationRecord>>>,
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
            records: Arc::new(RwLock::new(Vec::new())),
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
        let records = load_records(&path, retention).await?;
        Ok(Self {
            records: Arc::new(RwLock::new(records)),
            path: Some(Arc::new(path)),
            retention,
        })
    }

    pub async fn list(&self) -> Vec<NotificationRecord> {
        self.records.read().await.clone()
    }

    pub async fn push_if_new(&self, record: NotificationRecord) -> Result<bool> {
        let snapshot = {
            let mut records = self.records.write().await;
            if records
                .iter()
                .any(|existing| existing.dedupe_key == record.dedupe_key)
            {
                return Ok(false);
            }
            records.insert(0, record);
            records.truncate(self.retention);
            records.clone()
        };
        self.persist(&snapshot).await?;
        Ok(true)
    }

    async fn persist(&self, records: &[NotificationRecord]) -> Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        let body = serde_json::to_vec_pretty(records).context("serialize notifications")?;
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

async fn load_records(path: &Path, retention: usize) -> Result<Vec<NotificationRecord>> {
    let raw = match tokio::fs::read(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut records: Vec<NotificationRecord> =
        serde_json::from_slice(&raw).context("parse notifications")?;
    records.truncate(retention);
    Ok(records)
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

    let apprise = AppriseTarget::from_env();
    for source in sources {
        let source_id = source
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let label = source_label(source);
        let Some(history) = source.get("history").and_then(Value::as_array) else {
            continue;
        };
        for event in history.iter().rev() {
            if event.get("status").and_then(Value::as_str) != Some("failed") {
                continue;
            }
            let at = event.get("at").and_then(Value::as_str).unwrap_or("unknown");
            let job_id = event.get("jobId").and_then(Value::as_str).unwrap_or("");
            let phase = event
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("refresh");
            let error = event
                .get("error")
                .and_then(Value::as_str)
                .map(bounded_text)
                .unwrap_or_else(|| "unknown ingestion failure".to_string());
            let dedupe_key = format!("depot:{source_id}:{at}:{job_id}:{phase}");
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
                .push_if_new(record.clone())
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
                if let Some(target) = &apprise
                    && let Err(error) = target.send(&record).await
                {
                    tracing::warn!(
                        subsystem = "notifications",
                        source = "apprise",
                        error = %error,
                        "Apprise notification delivery failed"
                    );
                }
            }
        }
    }
    Ok(())
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
            .context("send Apprise notification")?;
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
