//! Durable authorization and exactly-once control for delegated agent calls.
//!
//! Opaque bearer material is stored only as SHA-256 digests. Audit rows contain
//! stable correlation identifiers and argument/contract hashes, never arguments,
//! credentials, delegation tokens, approval tokens, or upstream results.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use labby_runtime::error::ToolError;
mod manager;

const DELEGATION_TTL_MS: i64 = 5 * 60 * 1000;
const APPROVAL_TTL_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DelegationReceipt {
    pub delegation_token: String,
    pub actor: String,
    pub audience: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContextReceipt {
    pub execution_context_id: String,
    pub actor: String,
    pub service: String,
    pub loadout_id: String,
    pub loadout_revision: u64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalChallenge {
    pub approval_token: String,
    pub approval_id: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContextCreateRequest {
    pub delegation_token: String,
    pub loadout_id: String,
    pub loadout_revision: u64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalRequest {
    pub execution_context_id: String,
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub expected_contract_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecuteRequest {
    pub execution_context_id: String,
    pub idempotency_key: String,
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub expected_contract_hash: String,
    #[serde(default)]
    pub approval_token: Option<String>,
    pub deadline_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

impl AgentExecutionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentExecutionReceipt {
    pub request_id: String,
    pub receipt_id: String,
    pub audit_id: String,
    pub status: AgentExecutionStatus,
    pub tool_id: String,
    pub contract_hash: String,
    pub loadout_id: String,
    pub loadout_revision: u64,
    pub actor: String,
    pub service: String,
    pub execution_mode: super::palette::PaletteExecutionMode,
    pub llm_invocations: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BoundContext {
    pub actor: String,
    pub service: String,
    pub loadout_id: String,
    pub loadout_revision: u64,
    pub scopes: Vec<String>,
}

pub(crate) enum Reservation {
    Execute {
        receipt_id: String,
        audit_id: String,
    },
    Existing(AgentExecutionReceipt),
    Running(AgentExecutionReceipt),
}

pub struct AgentExecutionStore {
    connection: Mutex<Connection>,
}

impl AgentExecutionStore {
    pub fn open(path: PathBuf) -> Result<Self, ToolError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(storage_error)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                connection.path().expect("file-backed agent store"),
                std::fs::Permissions::from_mode(0o600),
            )
            .map_err(storage_error)?;
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(storage_error)?;
        connection.execute_batch(SCHEMA).map_err(storage_error)?;
        connection.execute(
            "UPDATE agent_requests SET status='interrupted', error_kind='interrupted' WHERE status='running'",
            [],
        ).map_err(storage_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    #[cfg(any(test, feature = "testkit"))]
    pub(crate) fn open_in_memory() -> Result<Self, ToolError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        connection.execute_batch(SCHEMA).map_err(storage_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn issue_delegation(
        &self,
        actor: &str,
        audience: &str,
        scopes: &[String],
    ) -> Result<DelegationReceipt, ToolError> {
        validate_identity(actor)?;
        validate_identity(audience)?;
        let token = opaque_token("dlg");
        let expires = now_ms() + DELEGATION_TTL_MS;
        self.conn()?.execute(
            "INSERT INTO agent_delegations(token_hash,actor,audience,scopes_json,expires_at,used_at) VALUES(?1,?2,?3,?4,?5,NULL)",
            params![digest(&token), actor, audience, serde_json::to_string(scopes).map_err(storage_error)?, expires],
        ).map_err(storage_error)?;
        Ok(DelegationReceipt {
            delegation_token: token,
            actor: actor.into(),
            audience: audience.into(),
            expires_at_unix_ms: expires,
        })
    }

    pub fn create_context(
        &self,
        service: &str,
        delegation: &str,
        loadout_id: &str,
        loadout_revision: u64,
        expires_at: i64,
    ) -> Result<ExecutionContextReceipt, ToolError> {
        validate_identity(service)?;
        validate_identity(loadout_id)?;
        let now = now_ms();
        if expires_at <= now {
            return Err(policy("execution context is already expired"));
        }
        let context_id = opaque_token("ctx");
        let conn = self.conn()?;
        let (actor, scopes_json): (String, String) = conn.query_row(
            "SELECT actor,scopes_json FROM agent_delegations WHERE token_hash=?1 AND audience=?2 AND used_at IS NULL AND expires_at>?3",
            params![digest(delegation), service, now], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(storage_error)?.ok_or_else(|| policy("delegation is invalid, stale, forged, used, or audience-mismatched"))?;
        let changed = conn.execute(
            "UPDATE agent_delegations SET used_at=?1 WHERE token_hash=?2 AND used_at IS NULL AND expires_at>?1",
            params![now, digest(delegation)],
        ).map_err(storage_error)?;
        if changed != 1 {
            return Err(policy("delegation was already consumed"));
        }
        conn.execute(
            "INSERT INTO agent_contexts(id_hash,actor,service,scopes_json,loadout_id,loadout_revision,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![digest(&context_id), actor, service, scopes_json, loadout_id, loadout_revision as i64, expires_at],
        ).map_err(storage_error)?;
        Ok(ExecutionContextReceipt {
            execution_context_id: context_id,
            actor,
            service: service.into(),
            loadout_id: loadout_id.into(),
            loadout_revision,
            expires_at_unix_ms: expires_at,
        })
    }

    pub(crate) fn bound_context(
        &self,
        context_id: &str,
        service: &str,
    ) -> Result<BoundContext, ToolError> {
        self.conn()?.query_row(
            "SELECT actor,service,loadout_id,loadout_revision,scopes_json FROM agent_contexts WHERE id_hash=?1 AND service=?2 AND expires_at>?3",
            params![digest(context_id), service, now_ms()],
            |row| { let scopes_json: String = row.get(4)?; Ok(BoundContext { actor: row.get(0)?, service: row.get(1)?, loadout_id: row.get(2)?, loadout_revision: row.get::<_, i64>(3)? as u64, scopes: serde_json::from_str(&scopes_json).unwrap_or_default() }) },
        ).optional().map_err(storage_error)?.ok_or_else(|| policy("execution context is invalid, expired, or service-mismatched"))
    }

    pub(crate) fn bound_context_for_actor(
        &self,
        context_id: &str,
        actor: &str,
    ) -> Result<BoundContext, ToolError> {
        let context = self.conn()?.query_row(
            "SELECT actor,service,loadout_id,loadout_revision,scopes_json FROM agent_contexts WHERE id_hash=?1 AND actor=?2 AND expires_at>?3",
            params![digest(context_id), actor, now_ms()],
            |row| { let scopes_json: String = row.get(4)?; Ok(BoundContext { actor: row.get(0)?, service: row.get(1)?, loadout_id: row.get(2)?, loadout_revision: row.get::<_, i64>(3)? as u64, scopes: serde_json::from_str(&scopes_json).unwrap_or_default() }) },
        ).optional().map_err(storage_error)?;
        context.ok_or_else(|| policy("execution context is invalid, expired, or actor-mismatched"))
    }

    pub fn issue_approval(
        &self,
        context_id: &str,
        service: &str,
        tool_id: &str,
        args_hash: &str,
        contract_hash: &str,
    ) -> Result<ApprovalChallenge, ToolError> {
        let context = self.bound_context(context_id, service)?;
        let token = opaque_token("apr");
        let id = Uuid::new_v4().to_string();
        let expires = now_ms() + APPROVAL_TTL_MS;
        self.conn()?.execute(
            "INSERT INTO agent_approvals(id,token_hash,context_hash,actor,service,loadout_id,loadout_revision,tool_id,args_hash,contract_hash,expires_at,used_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL)",
            params![id, digest(&token), digest(context_id), context.actor, service, context.loadout_id, context.loadout_revision as i64, tool_id, args_hash, contract_hash, expires],
        ).map_err(storage_error)?;
        Ok(ApprovalChallenge {
            approval_token: token,
            approval_id: id,
            expires_at_unix_ms: expires,
        })
    }

    pub(crate) fn reserve(
        &self,
        context_id: &str,
        service: &str,
        idempotency_key: &str,
        tool_id: &str,
        args_hash: &str,
        contract_hash: &str,
        approval: Option<&str>,
        destructive: bool,
    ) -> Result<Reservation, ToolError> {
        validate_identity(idempotency_key)?;
        let context = self.bound_context(context_id, service)?;
        let fingerprint = request_fingerprint(&context, tool_id, args_hash, contract_hash);
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(storage_error)?;
        if let Some(existing) = load_receipt(&tx, idempotency_key)? {
            if existing.0 != fingerprint {
                return Err(policy(
                    "idempotency key was reused with a different execution request",
                ));
            }
            return Ok(if existing.1.status == AgentExecutionStatus::Running {
                Reservation::Running(existing.1)
            } else {
                Reservation::Existing(existing.1)
            });
        }
        if destructive {
            let token =
                approval.ok_or_else(|| policy("a server-issued approval challenge is required"))?;
            let changed = tx.execute(
                "UPDATE agent_approvals SET used_at=?1 WHERE token_hash=?2 AND context_hash=?3 AND actor=?4 AND service=?5 AND loadout_id=?6 AND loadout_revision=?7 AND tool_id=?8 AND args_hash=?9 AND contract_hash=?10 AND used_at IS NULL AND expires_at>?1",
                params![now_ms(), digest(token), digest(context_id), context.actor, service, context.loadout_id, context.loadout_revision as i64, tool_id, args_hash, contract_hash],
            ).map_err(storage_error)?;
            if changed != 1 {
                return Err(policy(
                    "approval is stale, forged, mismatched, expired, or already used",
                ));
            }
        }
        let receipt_id = Uuid::new_v4().to_string();
        let audit_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO agent_requests(idempotency_key,fingerprint,receipt_id,audit_id,status,actor,service,loadout_id,loadout_revision,tool_id,args_hash,contract_hash,result_json,error_kind,created_at,updated_at) VALUES(?1,?2,?3,?4,'running',?5,?6,?7,?8,?9,?10,?11,NULL,NULL,?12,?12)",
            params![idempotency_key, fingerprint, receipt_id, audit_id, context.actor, service, context.loadout_id, context.loadout_revision as i64, tool_id, args_hash, contract_hash, now_ms()],
        ).map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(Reservation::Execute {
            receipt_id,
            audit_id,
        })
    }

    pub(crate) fn finish(
        &self,
        key: &str,
        status: AgentExecutionStatus,
        result: Option<&serde_json::Value>,
        error_kind: Option<&str>,
    ) -> Result<AgentExecutionReceipt, ToolError> {
        let result_json = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(storage_error)?;
        self.conn()?.execute(
            "UPDATE agent_requests SET status=?1,result_json=?2,error_kind=?3,updated_at=?4 WHERE idempotency_key=?5 AND status='running'",
            params![status.as_str(), result_json, error_kind, now_ms(), key],
        ).map_err(storage_error)?;
        self.status(key)?
            .ok_or_else(|| storage_error("execution receipt disappeared"))
    }

    pub fn status(&self, key: &str) -> Result<Option<AgentExecutionReceipt>, ToolError> {
        let conn = self.conn()?;
        Ok(load_receipt(&conn, key)?.map(|(_, receipt)| receipt))
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, ToolError> {
        self.connection
            .lock()
            .map_err(|_| storage_error("agent execution store lock poisoned"))
    }
}

fn load_receipt(
    conn: &Connection,
    key: &str,
) -> Result<Option<(String, AgentExecutionReceipt)>, ToolError> {
    conn.query_row("SELECT fingerprint,receipt_id,audit_id,status,actor,service,loadout_id,loadout_revision,tool_id,contract_hash,result_json,error_kind FROM agent_requests WHERE idempotency_key=?1", [key], |row| {
        let status: String = row.get(3)?;
        let result_json: Option<String> = row.get(10)?;
        Ok((row.get(0)?, AgentExecutionReceipt { request_id: key.into(), receipt_id: row.get(1)?, audit_id: row.get(2)?, status: parse_status(&status), actor: row.get(4)?, service: row.get(5)?, loadout_id: row.get(6)?, loadout_revision: row.get::<_, i64>(7)? as u64, tool_id: row.get(8)?, contract_hash: row.get(9)?, execution_mode: super::palette::PaletteExecutionMode::Exact, llm_invocations: 0, result: result_json.and_then(|v| serde_json::from_str(&v).ok()), error_kind: row.get(11)? }))
    }).optional().map_err(storage_error)
}

fn parse_status(value: &str) -> AgentExecutionStatus {
    match value {
        "running" => AgentExecutionStatus::Running,
        "succeeded" => AgentExecutionStatus::Succeeded,
        "failed" => AgentExecutionStatus::Failed,
        "cancelled" => AgentExecutionStatus::Cancelled,
        "timed_out" => AgentExecutionStatus::TimedOut,
        _ => AgentExecutionStatus::Interrupted,
    }
}
fn request_fingerprint(context: &BoundContext, tool: &str, args: &str, contract: &str) -> String {
    digest(&format!(
        "{}\0{}\0{}\0{}\0{}\0{}",
        context.actor,
        context.service,
        context.loadout_id,
        context.loadout_revision,
        tool,
        digest(&format!("{args}\0{contract}"))
    ))
}
pub(crate) fn canonical_args_hash(value: &serde_json::Value) -> Result<String, ToolError> {
    serde_json::to_vec(value)
        .map(|v| digest_bytes(&v))
        .map_err(storage_error)
}
fn opaque_token(prefix: &str) -> String {
    format!(
        "{prefix}_{}_{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}
fn digest(value: &str) -> String {
    digest_bytes(value.as_bytes())
}
fn digest_bytes(value: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value)))
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}
fn validate_identity(value: &str) -> Result<(), ToolError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(policy(
            "identifier is empty, too long, or contains control characters",
        ))
    } else {
        Ok(())
    }
}
fn policy(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "forbidden".into(),
        message: message.into(),
    }
}
fn storage_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal".into(),
        message: format!("agent execution storage failed: {error}"),
    }
}

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS agent_delegations(token_hash TEXT PRIMARY KEY,actor TEXT NOT NULL,audience TEXT NOT NULL,scopes_json TEXT NOT NULL,expires_at INTEGER NOT NULL,used_at INTEGER);
CREATE TABLE IF NOT EXISTS agent_contexts(id_hash TEXT PRIMARY KEY,actor TEXT NOT NULL,service TEXT NOT NULL,scopes_json TEXT NOT NULL,loadout_id TEXT NOT NULL,loadout_revision INTEGER NOT NULL,expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS agent_approvals(id TEXT PRIMARY KEY,token_hash TEXT UNIQUE NOT NULL,context_hash TEXT NOT NULL,actor TEXT NOT NULL,service TEXT NOT NULL,loadout_id TEXT NOT NULL,loadout_revision INTEGER NOT NULL,tool_id TEXT NOT NULL,args_hash TEXT NOT NULL,contract_hash TEXT NOT NULL,expires_at INTEGER NOT NULL,used_at INTEGER);
CREATE TABLE IF NOT EXISTS agent_requests(idempotency_key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,receipt_id TEXT UNIQUE NOT NULL,audit_id TEXT UNIQUE NOT NULL,status TEXT NOT NULL,actor TEXT NOT NULL,service TEXT NOT NULL,loadout_id TEXT NOT NULL,loadout_revision INTEGER NOT NULL,tool_id TEXT NOT NULL,args_hash TEXT NOT NULL,contract_hash TEXT NOT NULL,result_json TEXT,error_kind TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
";

#[cfg(test)]
#[path = "agent_execution_tests.rs"]
mod tests;
