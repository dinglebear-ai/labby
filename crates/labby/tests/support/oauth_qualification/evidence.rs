use std::{collections::BTreeMap, io::Read as _, path::Path};

use axum::http::{StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActualProviderGate {
    Available,
    Unavailable { missing: Vec<&'static str> },
}

pub(crate) fn actual_provider_gate(
    provider: &'static str,
    vars: &BTreeMap<String, String>,
) -> ActualProviderGate {
    let required: &[&str] = match provider {
        "google" => &["LABBY_Q2_GOOGLE_CLIENT_ID", "LABBY_Q2_GOOGLE_CLIENT_SECRET"],
        "authelia" => &[
            "LABBY_Q2_AUTHELIA_ISSUER",
            "LABBY_Q2_AUTHELIA_CLIENT_ID",
            "LABBY_Q2_AUTHELIA_CLIENT_SECRET",
        ],
        "github-upstream" => &[
            "LABBY_Q2_GITHUB_MCP_ENDPOINT",
            "LABBY_Q2_GITHUB_CLIENT_ID",
            "LABBY_Q2_GITHUB_CLIENT_SECRET",
        ],
        _ => {
            return ActualProviderGate::Unavailable {
                missing: vec!["unsupported-provider"],
            };
        }
    };
    let missing = required
        .iter()
        .copied()
        .filter(|name| vars.get(*name).is_none_or(|value| value.trim().is_empty()))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        ActualProviderGate::Available
    } else {
        ActualProviderGate::Unavailable { missing }
    }
}

pub(crate) async fn protected_initialize(
    base: &str,
    token: Option<&str>,
) -> Result<(StatusCode, Vec<u8>), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client
        .post(format!("{base}/operator"))
        .header(header::HOST, "mcp.example.test")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"q2","version":"1"}}
        }));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| error.to_string())?
        .to_vec();
    Ok((status, body))
}

pub(crate) async fn denied_revoke(
    base: &str,
    token: Option<&str>,
    credential_id: &str,
) -> Result<(StatusCode, Vec<u8>), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.delete(format!("{base}/v1/access/credentials/{credential_id}"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| error.to_string())?
        .to_vec();
    Ok((status, body))
}

pub(crate) fn authority_state_digest(root: &Path) -> Result<[u8; 32], String> {
    const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_TABLES: usize = 128;
    const MAX_COLUMNS: usize = 1_024;
    const MAX_ROWS: usize = 100_000;
    const MAX_FIELD_BYTES: usize = 4 * 1024 * 1024;
    const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

    struct SnapshotDigest {
        hasher: Sha256,
        bytes: usize,
    }

    impl SnapshotDigest {
        fn new() -> Self {
            let mut digest = Self {
                hasher: Sha256::new(),
                bytes: 0,
            };
            digest
                .tagged_bytes(0, b"labby-authority-semantic-snapshot-v1")
                .expect("domain separator fits the snapshot bound");
            digest
        }

        fn reserve(&mut self, additional: usize) -> Result<(), String> {
            self.bytes = self
                .bytes
                .checked_add(additional)
                .filter(|bytes| *bytes <= MAX_SNAPSHOT_BYTES)
                .ok_or_else(|| {
                    format!("authority snapshot exceeds {MAX_SNAPSHOT_BYTES} encoded bytes")
                })?;
            Ok(())
        }

        fn tag(&mut self, tag: u8) -> Result<(), String> {
            self.reserve(1)?;
            self.hasher.update([tag]);
            Ok(())
        }

        fn tagged_bytes(&mut self, tag: u8, bytes: &[u8]) -> Result<(), String> {
            self.reserve(1 + size_of::<u64>() + bytes.len())?;
            self.hasher.update([tag]);
            self.hasher.update((bytes.len() as u64).to_le_bytes());
            self.hasher.update(bytes);
            Ok(())
        }

        fn tagged_u64(&mut self, tag: u8, value: u64) -> Result<(), String> {
            self.reserve(1 + size_of::<u64>())?;
            self.hasher.update([tag]);
            self.hasher.update(value.to_le_bytes());
            Ok(())
        }

        fn finish(self) -> [u8; 32] {
            self.hasher.finalize().into()
        }
    }

    fn bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > limit {
            return Err(format!(
                "authority snapshot file {} exceeds {limit} bytes",
                path.display()
            ));
        }
        Ok(bytes)
    }

    let mut digest = SnapshotDigest::new();
    let config = root.join("labby-home/config.toml");
    if config.exists() {
        digest.tagged_bytes(2, &bounded_file(&config, MAX_CONFIG_BYTES)?)?;
    } else {
        digest.tag(1)?;
    }
    let database = root.join("labby-home/access.db");
    if database.exists() {
        digest.tag(4)?;
        let mut connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| error.to_string())?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
        // Credential admission and deny auditing are security enforcement side
        // effects, not changes to the caller's authorization. Foreign but
        // parseable credentials intentionally update these two operational
        // tables before the protected action is rejected. Keep every authority,
        // credential, session, token, and policy table in the semantic digest.
        let mut tables = transaction
            .prepare(
                "SELECT name,sql FROM sqlite_schema
                      WHERE type='table' AND name NOT LIKE 'sqlite_%'
                        AND name NOT IN ('access_admission_buckets','access_security_events')
                      ORDER BY name",
            )
            .map_err(|error| error.to_string())?;
        let tables = tables
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        if tables.len() > MAX_TABLES {
            return Err(format!("authority snapshot exceeds {MAX_TABLES} tables"));
        }
        let mut row_count = 0usize;
        for (table, schema) in tables {
            if table.len() > MAX_FIELD_BYTES || schema.len() > MAX_FIELD_BYTES {
                return Err(format!(
                    "authority snapshot schema field exceeds {MAX_FIELD_BYTES} bytes"
                ));
            }
            digest.tag(5)?;
            digest.tagged_bytes(6, table.as_bytes())?;
            digest.tagged_bytes(7, schema.as_bytes())?;
            let quoted = table.replace('"', "\"\"");
            let probe = transaction
                .prepare(&format!("SELECT * FROM \"{quoted}\" LIMIT 0"))
                .map_err(|error| error.to_string())?;
            let column_count = probe.column_count();
            if column_count > MAX_COLUMNS {
                return Err(format!(
                    "authority snapshot exceeds {MAX_COLUMNS} columns in one table"
                ));
            }
            digest.tagged_u64(8, column_count as u64)?;
            let order = (1..=column_count)
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction
                .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order}"))
                .map_err(|error| error.to_string())?;
            let mut rows = statement.query([]).map_err(|error| error.to_string())?;
            while let Some(row) = rows.next().map_err(|error| error.to_string())? {
                row_count += 1;
                if row_count > MAX_ROWS {
                    return Err(format!("authority snapshot exceeds {MAX_ROWS} rows"));
                }
                digest.tag(9)?;
                for index in 0..column_count {
                    use rusqlite::types::ValueRef;
                    match row.get_ref(index).map_err(|error| error.to_string())? {
                        ValueRef::Null => digest.tag(10)?,
                        ValueRef::Integer(value) => {
                            digest.tagged_u64(11, value as u64)?;
                        }
                        ValueRef::Real(value) => {
                            digest.tagged_u64(12, value.to_bits())?;
                        }
                        ValueRef::Text(value) if value.len() <= MAX_FIELD_BYTES => {
                            digest.tagged_bytes(13, value)?;
                        }
                        ValueRef::Blob(value) if value.len() <= MAX_FIELD_BYTES => {
                            digest.tagged_bytes(14, value)?;
                        }
                        ValueRef::Text(_) | ValueRef::Blob(_) => {
                            return Err(format!(
                                "authority snapshot field exceeds {MAX_FIELD_BYTES} bytes"
                            ));
                        }
                    }
                }
                digest.tag(15)?;
            }
            digest.tag(16)?;
        }
        digest.tag(17)?;
    } else {
        digest.tag(3)?;
    }
    Ok(digest.finish())
}

pub(crate) fn assert_log_secret_absent(root: &Path, secrets: &[&str]) -> Result<(), String> {
    const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
    let path = root.join("stderr.log");
    let file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_LOG_BYTES {
        return Err(format!(
            "daemon stderr exceeded {MAX_LOG_BYTES}-byte review bound"
        ));
    }
    assert_secret_absent(&bytes, secrets);
    Ok(())
}

pub(crate) fn assert_secret_absent(body: &[u8], secrets: &[&str]) {
    for (index, secret) in secrets.iter().enumerate() {
        assert!(!secret.is_empty());
        assert!(
            !body
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "secret at review-list index {index} was reflected"
        );
    }
}

pub(crate) fn github_inbound_rejection() -> String {
    labby_auth::config::AuthConfigBuilder::new()
        .build_from_sources([
            ("LAB_AUTH_MODE".to_string(), "oauth".to_string()),
            ("LAB_AUTH_PROVIDER".to_string(), "github".to_string()),
        ])
        .expect_err("GitHub is not an inbound provider")
        .to_string()
}

pub(crate) fn support_decisions() -> Value {
    json!({
        "google_deterministic": {"status":"qualified", "flow":"browser_session", "refresh":"not_applicable"},
        "authelia_deterministic": {"status":"qualified", "flow":"browser_session", "refresh":"not_applicable"},
        "github_inbound": {"status":"unsupported", "basis":"closed provider enum: google or authelia"},
        "github_upstream": {"status":"unknown", "basis":"actual endpoint metadata and registration are not qualified"}
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn authority_snapshot_observes_uncheckpointed_wal_rows() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("labby-home");
        std::fs::create_dir(&home).unwrap();
        let connection = rusqlite::Connection::open(home.join("access.db")).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .execute_batch("CREATE TABLE authority(id TEXT PRIMARY KEY, value TEXT) STRICT;")
            .unwrap();
        let before = super::authority_state_digest(root.path()).unwrap();
        connection
            .execute("INSERT INTO authority VALUES('one','changed')", [])
            .unwrap();
        assert_ne!(super::authority_state_digest(root.path()).unwrap(), before);
    }

    #[test]
    fn authority_snapshot_frames_presence_and_sqlite_value_types() {
        let root = tempfile::tempdir().unwrap();
        let absent = super::authority_state_digest(root.path()).unwrap();
        let home = root.path().join("labby-home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("config.toml"), []).unwrap();
        let empty_config = super::authority_state_digest(root.path()).unwrap();
        assert_ne!(
            empty_config, absent,
            "absent and empty config must not collide"
        );

        let connection = rusqlite::Connection::open(home.join("access.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE authority(value ANY) STRICT; INSERT INTO authority VALUES('');",
            )
            .unwrap();
        let text = super::authority_state_digest(root.path()).unwrap();
        connection
            .execute("UPDATE authority SET value = x''", [])
            .unwrap();
        let blob = super::authority_state_digest(root.path()).unwrap();
        assert_ne!(text, blob, "text and blob encodings must not collide");
    }

    #[test]
    fn authority_snapshot_excludes_only_operational_denial_telemetry() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("labby-home");
        std::fs::create_dir(&home).unwrap();
        let connection = rusqlite::Connection::open(home.join("access.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE access_admission_buckets(value INTEGER) STRICT;
                 CREATE TABLE access_security_events(value INTEGER) STRICT;
                 CREATE TABLE project_credentials(value TEXT) STRICT;
                 INSERT INTO project_credentials VALUES('active');",
            )
            .unwrap();
        let before = super::authority_state_digest(root.path()).unwrap();
        connection
            .execute_batch(
                "INSERT INTO access_admission_buckets VALUES(1);
                 INSERT INTO access_security_events VALUES(1);",
            )
            .unwrap();
        assert_eq!(
            super::authority_state_digest(root.path()).unwrap(),
            before,
            "operational denial telemetry is not an authority mutation"
        );
        connection
            .execute("UPDATE project_credentials SET value='revoked'", [])
            .unwrap();
        assert_ne!(
            super::authority_state_digest(root.path()).unwrap(),
            before,
            "credential authority rows must remain covered"
        );
    }
}
