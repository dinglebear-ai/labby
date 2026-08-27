use std::path::Path;

use serde::{Deserialize, Serialize};

use super::live_labby::RunIdentity;

pub(crate) const EVIDENCE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceKind {
    Setup,
    Process,
    Readiness,
    Cleanup,
    Failure,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct EvidenceEvent {
    pub(crate) sequence: u64,
    pub(crate) timestamp_ms: u128,
    pub(crate) kind: EvidenceKind,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RunEvidence {
    pub(crate) schema_version: u32,
    pub(crate) identity: RunIdentity,
    pub(crate) events: Vec<EvidenceEvent>,
    pub(crate) reproduction: String,
}

impl RunEvidence {
    pub(crate) fn new(identity: RunIdentity) -> Self {
        let reproduction = format!(
            "LABBY_E2E_SEED={} cargo nextest run -p labby --test live_process_harness",
            identity.seed
        );
        Self {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            identity,
            events: Vec::new(),
            reproduction,
        }
    }

    pub(crate) fn push(&mut self, kind: EvidenceKind, message: impl Into<String>) {
        self.events.push(EvidenceEvent {
            sequence: self.events.len() as u64,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            kind,
            message: sanitize(&message.into()),
        });
    }

    pub(crate) fn write_atomic(&self, path: &Path) -> std::io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "evidence has no parent")
        })?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)
    }
}

pub(crate) fn sanitize(value: &str) -> String {
    let mut sanitized = value.replace('\0', "\\0");
    for prefix in ["token=", "token =", "secret=", "secret =", "authorization:"] {
        let mut cursor = 0;
        while let Some(relative) = sanitized[cursor..].to_ascii_lowercase().find(prefix) {
            let start = cursor + relative;
            let mut value_start = start + prefix.len();
            while sanitized[value_start..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            {
                value_start += sanitized[value_start..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8);
            }
            let end = sanitized[value_start..]
                .find(char::is_whitespace)
                .map_or(sanitized.len(), |offset| value_start + offset);
            sanitized.replace_range(value_start..end, "[REDACTED]");
            cursor = value_start + "[REDACTED]".len();
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitizer_removes_common_secret_forms() {
        let rendered = sanitize("token=canary secret=another Authorization: Bearer-value");
        assert!(!rendered.contains("canary"));
        assert!(!rendered.contains("another"));
        assert!(!rendered.contains("Bearer-value"));
    }
}
