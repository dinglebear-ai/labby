//! Disk cache of the Code Mode upstream catalog for one-shot CLI invocations.
//!
//! `labby gateway code exec` builds the `codemode.*` JS proxy from the upstream
//! tool catalog. The MCP surface refreshes that catalog from a long-lived pool,
//! but a one-shot CLI process would have to connect every configured stdio
//! upstream per invocation just to generate the proxy. This cache persists the
//! per-upstream tool lists (fingerprinted against the upstream config and
//! bounded by a TTL) so repeat CLI invocations connect no non-OAuth upstream
//! for proxy generation (OAuth upstreams are subject-scoped and never cached,
//! so they are probed each run); tool calls still resolve live via
//! `resolve_code_mode_upstream_tool`, so a stale cache can only omit or
//! over-offer `codemode.*` helpers — never execute against stale state.
//!
//! Concurrency model: one-shot invocations may read/write concurrently. Every
//! read-modify-write transaction is serialized both in-process and across Labby
//! processes through a sibling advisory lock file, then published by the shared
//! owner-only atomic writer. Any parse failure is treated as a cache miss.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fd_lock::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::upstream::types::UpstreamTool;
use labby_runtime::gateway_config::UpstreamConfig;

const CACHE_VERSION: u32 = 1;
static CACHE_WRITE_LOCK: Mutex<()> = Mutex::new(());
/// How long a cached upstream catalog stays valid. The fingerprint catches
/// config edits; the TTL catches upstream-side tool drift (server upgrades)
/// that no config change reflects.
#[allow(dead_code)]
const CACHE_TTL: Duration = Duration::from_hours(6);

/// Capped exponential suppression window for an upstream that failed to probe.
///
/// Deliberately minute-scale and separate from [`CACHE_TTL`]: pinning a failure
/// for six hours would make a transient outage look permanent, while dropping
/// it entirely makes every run re-pay a multi-second connect against an
/// upstream already known to be down.
///
/// Distinct from [`labby_runtime::backoff::reprobe_backoff`] on purpose: that
/// ladder paces in-process retries at second scale, this one paces suppression
/// across separate one-shot CLI invocations.
fn negative_backoff(consecutive_failures: u32) -> Duration {
    let seconds = match consecutive_failures {
        0 | 1 => 30,
        2 => 60,
        3 => 120,
        _ => 300,
    };
    Duration::from_secs(seconds)
}

/// Deterministic jitter seed for an upstream, so hosts sharing a dead upstream
/// do not resynchronise onto it. Deterministic rather than random keeps the
/// window reproducible under test.
fn jitter_seed(upstream_name: &str) -> u64 {
    let digest = Sha256::digest(upstream_name.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct CatalogCache {
    version: u32,
    upstreams: HashMap<String, CachedUpstreamCatalog>,
    /// Short-lived negative entries. `serde(default)` so a cache written by a
    /// build without them still loads.
    #[serde(default)]
    failures: HashMap<String, CachedUpstreamFailure>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedUpstreamCatalog {
    /// Fingerprint of the upstream config this entry was captured under.
    fingerprint: String,
    saved_at_unix: u64,
    tools: Vec<CachedTool>,
}

/// A recorded probe failure, suppressing reconnect attempts until
/// `retry_after_unix`. Bound to the config fingerprint so editing an upstream
/// retries it immediately instead of waiting out the backoff.
#[derive(Clone, Serialize, Deserialize)]
struct CachedUpstreamFailure {
    fingerprint: String,
    #[serde(default)]
    failed_at_unix: u64,
    #[serde(default)]
    consecutive_failures: u32,
    #[serde(default)]
    retry_after_unix: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedTool {
    tool: rmcp::model::Tool,
    input_schema: Option<Value>,
    output_schema: Option<Value>,
    destructive: bool,
}

/// A pending negative cache entry for one upstream whose probe failed.
///
/// Only genuine probe failures belong here. An upstream that was still
/// connecting, or was never attempted, when the cold-connect budget ended has
/// not failed — suppressing it would turn a slow run into a lasting outage.
pub(crate) struct CatalogCacheFailure {
    pub(crate) upstream_name: String,
    pub(crate) fingerprint: String,
}

/// A pending cache update for one upstream, produced after a live probe.
pub(crate) struct CatalogCacheUpdate {
    pub(crate) upstream_name: String,
    pub(crate) fingerprint: String,
    pub(crate) tools: Vec<UpstreamTool>,
}

pub(crate) fn cache_path() -> PathBuf {
    labby_runtime::lab_home()
        .join("cache")
        .join("codemode-catalog.json")
}

fn cache_lock_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("codemode-catalog.json");
    path.with_file_name(format!("{file_name}.lock"))
}

/// Stable fingerprint of an upstream config entry.
pub(crate) fn fingerprint(config: &UpstreamConfig) -> String {
    let serialized = serde_json::to_string(config).unwrap_or_else(|_| format!("{:?}", config.name));
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl CatalogCache {
    /// Load the cache at `path`. Missing, unreadable, corrupt, or
    /// version-mismatched files are all treated as an empty cache.
    pub(crate) fn load_from(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(error) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "code_mode.catalog_cache",
                    path = %path.display(),
                    error = %error,
                    "code_mode catalog cache unreadable; treating as empty"
                );
                return Self::default();
            }
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            Ok(_) | Err(_) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "code_mode.catalog_cache",
                    path = %path.display(),
                    "code_mode catalog cache corrupt or version-mismatched; treating as empty"
                );
                Self::default()
            }
        }
    }

    /// Return the cached tools for `upstream_name` when the entry matches
    /// `fingerprint` and is within the TTL.
    #[allow(dead_code)]
    pub(crate) fn fresh_tools(
        &self,
        upstream_name: &str,
        fingerprint: &str,
    ) -> Option<Vec<UpstreamTool>> {
        let entry = self.upstreams.get(upstream_name)?;
        if entry.fingerprint != fingerprint {
            return None;
        }
        let age = now_unix().checked_sub(entry.saved_at_unix)?;
        if age > CACHE_TTL.as_secs() {
            return None;
        }
        let name: Arc<str> = Arc::from(upstream_name);
        Some(
            entry
                .tools
                .iter()
                .cloned()
                .map(|cached| UpstreamTool {
                    tool: cached.tool,
                    input_schema: cached.input_schema,
                    output_schema: cached.output_schema,
                    upstream_name: Arc::clone(&name),
                    destructive: cached.destructive,
                })
                .collect(),
        )
    }

    /// Return `true` when `upstream_name` failed recently enough that this run
    /// should skip it instead of re-paying the connect.
    ///
    /// Suppression lapses on its own once `retry_after_unix` passes, and any
    /// config edit changes the fingerprint and clears it, so a recovered or
    /// reconfigured upstream can never stay invisible.
    pub(crate) fn probe_suppressed(&self, upstream_name: &str, fingerprint: &str) -> bool {
        let Some(entry) = self.failures.get(upstream_name) else {
            return false;
        };
        if entry.fingerprint != fingerprint {
            return false;
        }
        now_unix() < entry.retry_after_unix
    }
}

/// Merge `updates` into the on-disk cache at `path` and persist atomically.
///
/// Holds both the process-local writer gate and the sibling advisory file lock
/// across load, merge, and atomic persist, so concurrent invocations updating
/// different upstreams cannot clobber each other.
///
/// Upstreams that were still connecting when the caller's budget ended, or were
/// never attempted, must NOT be passed in either argument — they have not
/// failed, and leaving them absent means the next run retries them immediately.
///
/// Upstreams whose probe genuinely failed go in `failures`, not `updates`. They
/// are recorded as short-lived negative entries rather than dropped: dropping
/// them means every subsequent run re-pays a multi-second connect against an
/// upstream already known to be down, which is what lets a handful of dead
/// upstreams exhaust the cold-connect budget before healthy ones are reached. A
/// success clears any negative entry for that upstream.
///
/// The write is completed before returning so one-shot CLI invocations do not
/// exit before a refreshed cache lands on disk. The write is skipped entirely
/// only when no entry has changed and no TTL timestamp needs renewal.
pub(crate) async fn merge_and_store(
    path: PathBuf,
    updates: Vec<CatalogCacheUpdate>,
    failures: Vec<CatalogCacheFailure>,
) {
    if updates.is_empty() && failures.is_empty() {
        return;
    }
    if let Err(error) =
        tokio::task::spawn_blocking(move || merge_and_store_blocking(&path, updates, failures))
            .await
    {
        tracing::warn!(
            surface = "dispatch",
            service = "gateway",
            action = "code_mode.catalog_cache",
            error = %error,
            "failed to join code_mode catalog cache persistence task"
        );
    }
}

fn merge_and_store_blocking(
    path: &Path,
    updates: Vec<CatalogCacheUpdate>,
    failures: Vec<CatalogCacheFailure>,
) {
    if let Err(error) = merge_and_store_locked(path, updates, failures) {
        tracing::warn!(
            surface = "dispatch",
            service = "gateway",
            action = "code_mode.catalog_cache",
            path = %path.display(),
            error = %error,
            "failed to persist code_mode catalog cache"
        );
    }
}

fn merge_and_store_locked(
    path: &Path,
    updates: Vec<CatalogCacheUpdate>,
    failures: Vec<CatalogCacheFailure>,
) -> std::io::Result<()> {
    let _process_guard = CACHE_WRITE_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("code_mode catalog cache writer lock poisoned"))?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("cache path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;

    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache_lock_path(path))?;
    let mut file_lock = RwLock::new(lock_file);
    let _file_guard = file_lock.write()?;

    let mut cache = CatalogCache::load_from(path);
    cache.version = CACHE_VERSION;
    let saved_at_unix = now_unix();
    let mut changed = false;
    for update in updates {
        // A successful probe clears any suppression for that upstream.
        changed |= cache.failures.remove(&update.upstream_name).is_some();
        changed |= merge_update_into_cache(&mut cache, update, saved_at_unix);
    }
    for failure in failures {
        changed |= merge_failure_into_cache(&mut cache, failure, saved_at_unix);
    }
    changed |= prune_expired_failures(&mut cache, saved_at_unix);

    if changed {
        persist_atomic(path, &cache)?;
    }
    Ok(())
}

/// Record a probe failure, escalating the window when the same upstream fails
/// again under the same configuration.
fn merge_failure_into_cache(
    cache: &mut CatalogCache,
    failure: CatalogCacheFailure,
    saved_at_unix: u64,
) -> bool {
    let consecutive_failures = cache
        .failures
        .get(&failure.upstream_name)
        .filter(|existing| existing.fingerprint == failure.fingerprint)
        .map_or(1, |existing| {
            existing.consecutive_failures.saturating_add(1)
        });

    let window = labby_runtime::backoff::jitter_delay(
        negative_backoff(consecutive_failures),
        jitter_seed(&failure.upstream_name),
    );

    cache.failures.insert(
        failure.upstream_name,
        CachedUpstreamFailure {
            fingerprint: failure.fingerprint,
            failed_at_unix: saved_at_unix,
            consecutive_failures,
            retry_after_unix: saved_at_unix.saturating_add(window.as_secs()),
        },
    );
    true
}

/// Drop negative entries whose window has lapsed, so the file does not
/// accumulate a row per upstream ever seen failing.
fn prune_expired_failures(cache: &mut CatalogCache, now_unix: u64) -> bool {
    let before = cache.failures.len();
    cache
        .failures
        .retain(|_, entry| now_unix < entry.retry_after_unix);
    cache.failures.len() != before
}

fn merge_update_into_cache(
    cache: &mut CatalogCache,
    update: CatalogCacheUpdate,
    saved_at_unix: u64,
) -> bool {
    let tools: Vec<CachedTool> = update
        .tools
        .into_iter()
        .map(|tool| CachedTool {
            tool: tool.tool,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
            destructive: tool.destructive,
        })
        .collect();

    let unchanged = cache
        .upstreams
        .get(&update.upstream_name)
        .is_some_and(|existing| {
            existing.fingerprint == update.fingerprint
                && existing.saved_at_unix == saved_at_unix
                && tools_fingerprint_matches(existing, &tools)
        });
    if unchanged {
        return false;
    }

    cache.upstreams.insert(
        update.upstream_name,
        CachedUpstreamCatalog {
            fingerprint: update.fingerprint,
            saved_at_unix,
            tools,
        },
    );
    true
}

/// Returns `true` when the serialised tool list of `existing` matches `tools`.
///
/// Using serialised JSON as the comparison key is cheap enough for the small
/// catalogs stored here and avoids a bespoke `PartialEq` impl on the rmcp
/// `Tool` type.
fn tools_fingerprint_matches(existing: &CachedUpstreamCatalog, tools: &[CachedTool]) -> bool {
    if existing.tools.len() != tools.len() {
        return false;
    }
    // Quick serialised-hash comparison: serialize both sides and compare bytes.
    let existing_bytes = serde_json::to_vec(&existing.tools).unwrap_or_default();
    let new_bytes = serde_json::to_vec(tools).unwrap_or_default();
    existing_bytes == new_bytes
}

/// Publish the cache through the shared owner-only atomic writer.
///
/// The cache is derived from `config.toml` — each entry is keyed by a digest of
/// the whole serialized upstream config, and the tool descriptions come from
/// the upstreams themselves — so it must not be more readable than the config
/// it mirrors (`0600`). `write_secure_atomic` also names its own temporary
/// file, which a process-id-based name did not: two concurrent writers in one
/// process (a `snippets.exec` and a Code Mode refresh both reach here) could
/// otherwise interleave on the same temp path and publish a truncated cache.
fn persist_atomic(path: &Path, cache: &CatalogCache) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("cache path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(cache).map_err(std::io::Error::other)?;
    labby_runtime::secure_atomic_file::write_secure_atomic(path, &bytes)
        .map_err(|error| error.source)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use super::*;

    fn test_tool(name: &str) -> UpstreamTool {
        UpstreamTool {
            tool: rmcp::model::Tool::new(
                name.to_string(),
                "test tool",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: Some(serde_json::json!({"type": "object"})),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: true,
        }
    }

    fn typed_output_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" },
                "message": { "type": "string" }
            },
            "required": ["ok"],
            "additionalProperties": false
        })
    }

    #[test]
    fn fresh_tools_round_trips_through_serde() {
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
            failures: HashMap::new(),
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: now_unix(),
                tools: vec![CachedTool {
                    tool: test_tool("ping").tool,
                    input_schema: Some(serde_json::json!({"type": "object"})),
                    output_schema: None,
                    destructive: true,
                }],
            },
        );

        let bytes = serde_json::to_vec(&cache).expect("serializes");
        let restored: CatalogCache = serde_json::from_slice(&bytes).expect("deserializes");
        let tools = restored.fresh_tools("alpha", "fp").expect("entry is fresh");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool.name.as_ref(), "ping");
        assert!(tools[0].destructive, "destructive flag survives the cache");
        assert_eq!(tools[0].upstream_name.as_ref(), "alpha");
    }

    #[test]
    fn fresh_tools_round_trips_output_schema_through_serde() {
        let output_schema = typed_output_schema();
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
            failures: HashMap::new(),
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: now_unix(),
                tools: vec![CachedTool {
                    tool: test_tool("typed").tool,
                    input_schema: Some(serde_json::json!({"type": "object"})),
                    output_schema: Some(output_schema.clone()),
                    destructive: false,
                }],
            },
        );

        let bytes = serde_json::to_vec(&cache).expect("serializes");
        let restored: CatalogCache = serde_json::from_slice(&bytes).expect("deserializes");
        let tools = restored.fresh_tools("alpha", "fp").expect("entry is fresh");

        assert_eq!(tools[0].output_schema, Some(output_schema));
    }

    #[test]
    fn cache_update_stores_output_schema_for_future_cli_catalogs() {
        let mut tool = test_tool("typed");
        let output_schema = typed_output_schema();
        tool.output_schema = Some(output_schema.clone());
        let update = CatalogCacheUpdate {
            upstream_name: "alpha".to_string(),
            fingerprint: "fp".to_string(),
            tools: vec![tool],
        };

        let cached: Vec<CachedTool> = update
            .tools
            .into_iter()
            .map(|tool| CachedTool {
                tool: tool.tool,
                input_schema: tool.input_schema,
                output_schema: tool.output_schema,
                destructive: tool.destructive,
            })
            .collect();

        assert_eq!(cached[0].output_schema, Some(output_schema));
    }

    #[test]
    fn fresh_tools_rejects_fingerprint_mismatch_and_expired_entries() {
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
            failures: HashMap::new(),
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: now_unix() - CACHE_TTL.as_secs() - 60,
                tools: Vec::new(),
            },
        );
        cache.upstreams.insert(
            "beta".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "old-fp".to_string(),
                saved_at_unix: now_unix(),
                tools: Vec::new(),
            },
        );

        assert!(cache.fresh_tools("alpha", "fp").is_none(), "expired");
        assert!(
            cache.fresh_tools("beta", "new-fp").is_none(),
            "fingerprint mismatch"
        );
        assert!(cache.fresh_tools("missing", "fp").is_none());
    }

    #[test]
    fn identical_update_renews_saved_at_so_expired_entries_become_fresh() {
        let mut cache = CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
            failures: HashMap::new(),
        };
        let stale_saved_at = now_unix() - CACHE_TTL.as_secs() - 60;
        let tool = CachedTool {
            tool: test_tool("ping").tool,
            input_schema: Some(serde_json::json!({"type": "object"})),
            output_schema: None,
            destructive: false,
        };
        cache.upstreams.insert(
            "alpha".to_string(),
            CachedUpstreamCatalog {
                fingerprint: "fp".to_string(),
                saved_at_unix: stale_saved_at,
                tools: vec![tool.clone()],
            },
        );

        let changed = merge_update_into_cache(
            &mut cache,
            CatalogCacheUpdate {
                upstream_name: "alpha".to_string(),
                fingerprint: "fp".to_string(),
                tools: vec![UpstreamTool {
                    tool: tool.tool,
                    input_schema: tool.input_schema,
                    output_schema: tool.output_schema,
                    upstream_name: Arc::from("alpha"),
                    destructive: tool.destructive,
                }],
            },
            now_unix(),
        );

        assert!(changed, "TTL renewal must persist even when tools match");
        let entry = cache.upstreams.get("alpha").expect("cache entry");
        assert!(
            entry.saved_at_unix > stale_saved_at,
            "saved_at_unix should be renewed"
        );
        assert!(
            cache.fresh_tools("alpha", "fp").is_some(),
            "renewed entry should be fresh"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_config_sensitive() {
        let config = UpstreamConfig {
            enabled: true,
            name: "alpha".to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        let mut changed = config.clone();
        changed.args = vec!["--flag".to_string()];

        assert_eq!(fingerprint(&config), fingerprint(&config));
        assert_ne!(fingerprint(&config), fingerprint(&changed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_writers_preserve_distinct_upstream_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode-catalog.json");

        tokio::join!(
            merge_and_store(
                path.clone(),
                vec![CatalogCacheUpdate {
                    upstream_name: "alpha".to_string(),
                    fingerprint: "fp-alpha".to_string(),
                    tools: vec![test_tool("ping")],
                }],
                Vec::new(),
            ),
            merge_and_store(
                path.clone(),
                vec![CatalogCacheUpdate {
                    upstream_name: "beta".to_string(),
                    fingerprint: "fp-beta".to_string(),
                    tools: vec![test_tool("pong")],
                }],
                Vec::new(),
            ),
        );

        let cache = CatalogCache::load_from(&path);
        assert_eq!(
            cache
                .fresh_tools("alpha", "fp-alpha")
                .expect("alpha update")[0]
                .tool
                .name
                .as_ref(),
            "ping"
        );
        assert_eq!(
            cache.fresh_tools("beta", "fp-beta").expect("beta update")[0]
                .tool
                .name
                .as_ref(),
            "pong"
        );
    }

    #[test]
    fn sibling_file_lock_excludes_an_independent_writer_handle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode-catalog.json");
        let lock_path = cache_lock_path(&path);
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .expect("open cache lock")
        };
        let mut first = RwLock::new(open());
        let mut second = RwLock::new(open());

        let guard = first.try_write().expect("first writer acquires lock");
        assert!(
            second.try_write().is_err(),
            "independent writer must observe the advisory lock"
        );
        drop(guard);
        assert!(
            second.try_write().is_ok(),
            "writer may acquire after the first guard is released"
        );
    }

    /// The cache mirrors `config.toml` (its keys are digests of the serialized
    /// upstream config, its values the upstreams' own tool descriptions), so it
    /// must not be readable by anyone who cannot already read that config.
    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_cache_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cache").join("codemode-catalog.json");
        merge_and_store(
            path.clone(),
            vec![CatalogCacheUpdate {
                upstream_name: "alpha".to_string(),
                fingerprint: "fp".to_string(),
                tools: Vec::new(),
            }],
            Vec::new(),
        )
        .await;

        let mode = std::fs::metadata(&path)
            .expect("cache written")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "cache must not be group- or world-readable");
    }

    fn empty_cache() -> CatalogCache {
        CatalogCache {
            version: CACHE_VERSION,
            upstreams: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    fn failure(name: &str, fingerprint: &str) -> CatalogCacheFailure {
        CatalogCacheFailure {
            upstream_name: name.to_string(),
            fingerprint: fingerprint.to_string(),
        }
    }

    #[test]
    fn recorded_failure_suppresses_the_next_probe() {
        let mut cache = empty_cache();
        assert!(!cache.probe_suppressed("alpha", "fp"), "clean cache");
        assert!(merge_failure_into_cache(
            &mut cache,
            failure("alpha", "fp"),
            now_unix()
        ));
        assert!(cache.probe_suppressed("alpha", "fp"));
        assert!(!cache.probe_suppressed("beta", "fp"), "per-upstream");
    }

    #[test]
    fn suppression_lapses_once_the_window_passes() {
        let mut cache = empty_cache();
        let long_ago = now_unix() - negative_backoff(1).as_secs() * 2;
        merge_failure_into_cache(&mut cache, failure("alpha", "fp"), long_ago);
        assert!(
            !cache.probe_suppressed("alpha", "fp"),
            "an expired negative entry must not keep an upstream invisible"
        );
    }

    #[test]
    fn repeated_failures_escalate_the_window_and_cap() {
        let mut cache = empty_cache();
        let now = now_unix();
        let mut windows = Vec::new();
        for _ in 0..6 {
            merge_failure_into_cache(&mut cache, failure("alpha", "fp"), now);
            windows.push(cache.failures["alpha"].retry_after_unix - now);
        }
        assert_eq!(cache.failures["alpha"].consecutive_failures, 6);
        assert!(
            windows[0] <= windows[1] && windows[1] <= windows[2],
            "window should escalate: {windows:?}"
        );
        let (_, max_jitter) = labby_runtime::backoff::jitter_window(negative_backoff(u32::MAX));
        assert!(
            windows.iter().all(|w| *w <= max_jitter.as_secs()),
            "window must stay capped: {windows:?}"
        );
    }

    #[test]
    fn changing_upstream_config_retries_immediately() {
        let mut cache = empty_cache();
        merge_failure_into_cache(&mut cache, failure("alpha", "fp"), now_unix());
        assert!(cache.probe_suppressed("alpha", "fp"));
        assert!(
            !cache.probe_suppressed("alpha", "edited-fp"),
            "a config edit must clear suppression rather than wait out the backoff"
        );
    }

    #[test]
    fn a_successful_probe_clears_suppression_and_restores_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode-catalog.json");
        let now = now_unix();

        let mut seeded = empty_cache();
        seeded.version = CACHE_VERSION;
        merge_failure_into_cache(&mut seeded, failure("alpha", "fp"), now);
        persist_atomic(&path, &seeded).expect("seed cache");
        assert!(CatalogCache::load_from(&path).probe_suppressed("alpha", "fp"));

        // A tool that did not exist while the upstream was failing must still be
        // discovered on recovery.
        merge_and_store_blocking(
            &path,
            vec![CatalogCacheUpdate {
                upstream_name: "alpha".to_string(),
                fingerprint: "fp".to_string(),
                tools: vec![test_tool("brand_new_tool")],
            }],
            Vec::new(),
        );

        let reloaded = CatalogCache::load_from(&path);
        assert!(!reloaded.probe_suppressed("alpha", "fp"));
        let tools = reloaded
            .fresh_tools("alpha", "fp")
            .expect("recovered tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool.name, "brand_new_tool");
    }

    #[test]
    fn failures_round_trip_to_disk_and_expired_ones_are_pruned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode-catalog.json");

        merge_and_store_blocking(&path, Vec::new(), vec![failure("dead", "fp")]);
        assert!(CatalogCache::load_from(&path).probe_suppressed("dead", "fp"));

        let mut stale = empty_cache();
        stale.version = CACHE_VERSION;
        merge_failure_into_cache(
            &mut stale,
            failure("stale", "fp"),
            now_unix() - negative_backoff(1).as_secs() * 2,
        );
        assert!(prune_expired_failures(&mut stale, now_unix()));
        assert!(stale.failures.is_empty());
    }

    #[test]
    fn cache_written_without_negative_entries_still_loads() {
        let legacy = serde_json::json!({
            "version": CACHE_VERSION,
            "upstreams": {
                "alpha": { "fingerprint": "fp", "saved_at_unix": now_unix(), "tools": [] }
            }
        });
        let cache: CatalogCache =
            serde_json::from_value(legacy).expect("legacy cache must deserialize");
        assert!(cache.failures.is_empty());
        assert!(!cache.probe_suppressed("alpha", "fp"));
        assert!(cache.fresh_tools("alpha", "fp").is_some());
    }

    #[test]
    fn jitter_seed_is_stable_per_upstream_and_differs_across_upstreams() {
        assert_eq!(jitter_seed("alpha"), jitter_seed("alpha"));
        assert_ne!(jitter_seed("alpha"), jitter_seed("beta"));
    }
}
