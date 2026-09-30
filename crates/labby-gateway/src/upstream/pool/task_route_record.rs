//! Serializable routing metadata, independent of any live MCP peer.

use rmcp::model::Task;

use super::task_route::TaskRouteAuthorization;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TaskRouteRecord {
    pub(super) public_task_id: String,
    pub(super) native_task_id: String,
    pub(super) upstream_name: String,
    pub(super) caller_subject: Option<String>,
    pub(super) oauth_subject: Option<String>,
    pub(super) authorization: TaskRouteAuthorization,
    pub(super) config_fingerprint: String,
    pub(super) created_at_unix_ms: i64,
    pub(super) updated_at_unix_ms: i64,
    pub(super) ttl_ms: Option<u64>,
    pub(super) poll_interval_ms: Option<u64>,
}

// Owner references and native task identifiers must not escape through logs.
impl std::fmt::Debug for TaskRouteRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRouteRecord").finish_non_exhaustive()
    }
}

impl TaskRouteRecord {
    pub(super) fn validate(&self) -> Result<(), String> {
        if !valid_public_task_id(&self.public_task_id)
            || self.native_task_id.is_empty()
            || self.native_task_id.len() > 8192
            || self.upstream_name.is_empty()
            || self.upstream_name.len() > 1024
            || self.authorization.route_key.is_empty()
            || self.authorization.route_key.len() > 8192
            || self.config_fingerprint.len() != 64
            || !self
                .config_fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || self.caller_subject.as_ref().is_some_and(|s| s.len() > 8192)
            || self.oauth_subject.as_ref().is_some_and(|s| s.len() > 8192)
            || self
                .authorization
                .allowed_upstreams
                .as_ref()
                .is_some_and(|names| {
                    names.len() > 4096 || names.iter().any(|name| name.len() > 1024)
                })
            || self.created_at_unix_ms < 0
            || self.updated_at_unix_ms < self.created_at_unix_ms
        {
            return Err("invalid task route metadata".to_string());
        }
        self.expiry()?;
        if self
            .poll_interval_ms
            .is_some_and(|value| i64::try_from(value).is_err())
        {
            return Err("task poll interval exceeds storage range".to_string());
        }
        Ok(())
    }

    pub(super) fn same_binding(&self, other: &Self) -> bool {
        self.public_task_id == other.public_task_id
            && self.native_task_id == other.native_task_id
            && self.upstream_name == other.upstream_name
            && self.caller_subject == other.caller_subject
            && self.oauth_subject == other.oauth_subject
            && self.authorization == other.authorization
            && self.config_fingerprint == other.config_fingerprint
            && self.created_at_unix_ms == other.created_at_unix_ms
    }

    pub(super) fn authorized(
        &self,
        caller: Option<&str>,
        authorization: &TaskRouteAuthorization,
    ) -> bool {
        self.caller_subject.as_deref() == caller
            && self.authorization == *authorization
            && authorization
                .allowed_upstreams
                .as_ref()
                .is_none_or(|names| names.contains(&self.upstream_name))
    }

    fn expiry(&self) -> Result<Option<i64>, String> {
        self.ttl_ms
            .map(|ttl| {
                let ttl =
                    i64::try_from(ttl).map_err(|_| "task TTL exceeds storage range".to_string())?;
                self.created_at_unix_ms
                    .checked_add(ttl)
                    .ok_or_else(|| "task expiry exceeds storage range".to_string())
            })
            .transpose()
    }

    pub(super) fn expired(&self, now: i64) -> bool {
        match self.expiry() {
            Ok(Some(expiry)) => now >= expiry,
            Ok(None) => false,
            Err(_) => true,
        }
    }

    pub(super) fn observe(&mut self, task: &Task) -> Result<(), String> {
        let created = timestamp_millis(&task.created_at)?;
        let updated = timestamp_millis(&task.last_updated_at)?;
        if task.task_id != self.native_task_id || created != self.created_at_unix_ms {
            return Err("upstream task identity changed".to_string());
        }
        if updated < self.updated_at_unix_ms {
            return Err("stale upstream task observation".to_string());
        }
        self.updated_at_unix_ms = updated;
        self.ttl_ms = task.ttl_ms;
        self.poll_interval_ms = task.poll_interval_ms;
        self.validate()
    }
}

pub(super) fn timestamp_millis(value: &str) -> Result<i64, String> {
    value
        .parse::<jiff::Timestamp>()
        .map(jiff::Timestamp::as_millisecond)
        .map_err(|_| "invalid upstream task timestamp".to_string())
}

pub(super) fn valid_public_task_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("labby-task-") else {
        return false;
    };
    suffix.len() == 32
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && uuid::Uuid::parse_str(suffix).is_ok_and(|id| {
            id.get_version() == Some(uuid::Version::Random)
                && id.get_variant() == uuid::Variant::RFC4122
        })
}
