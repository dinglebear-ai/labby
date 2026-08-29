use super::*;
use std::sync::{Arc, Barrier};

fn store() -> (tempfile::TempDir, AgentExecutionStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = AgentExecutionStore::open(dir.path().join("agent.sqlite3")).unwrap();
    (dir, store)
}

fn context(store: &AgentExecutionStore) -> ExecutionContextReceipt {
    let delegation = store
        .issue_delegation(
            "actor-1",
            "axon-service",
            &[
                "mcp:read".into(),
                "mcp:write".into(),
                "gateway:github".into(),
            ],
        )
        .unwrap();
    store
        .create_context(
            "axon-service",
            &delegation.delegation_token,
            "loadout-1",
            7,
            now_ms() + 60_000,
        )
        .unwrap()
}

#[test]
fn delegation_is_audience_bound_single_use_and_forgery_safe() {
    let (_dir, store) = store();
    let delegation = store
        .issue_delegation("actor-1", "axon-service", &["mcp:write".into()])
        .unwrap();
    assert!(
        store
            .create_context(
                "other-service",
                &delegation.delegation_token,
                "l",
                1,
                now_ms() + 10_000
            )
            .is_err()
    );
    assert!(
        store
            .create_context("axon-service", "dlg_forged", "l", 1, now_ms() + 10_000)
            .is_err()
    );
    store
        .create_context(
            "axon-service",
            &delegation.delegation_token,
            "l",
            1,
            now_ms() + 10_000,
        )
        .unwrap();
    assert!(
        store
            .create_context(
                "axon-service",
                &delegation.delegation_token,
                "l",
                1,
                now_ms() + 10_000
            )
            .is_err()
    );
}

#[test]
fn stale_delegation_and_context_fail_closed() {
    let (_dir, store) = store();
    let delegation = store.issue_delegation("actor", "svc", &[]).unwrap();
    store
        .conn()
        .unwrap()
        .execute("UPDATE agent_delegations SET expires_at=?1", [now_ms() - 1])
        .unwrap();
    assert!(
        store
            .create_context(
                "svc",
                &delegation.delegation_token,
                "l",
                1,
                now_ms() + 1_000
            )
            .is_err()
    );

    let context = context(&store);
    store
        .conn()
        .unwrap()
        .execute("UPDATE agent_contexts SET expires_at=?1", [now_ms() - 1])
        .unwrap();
    assert!(
        store
            .bound_context(&context.execution_context_id, "axon-service")
            .is_err()
    );
}

#[test]
fn context_cannot_outlive_delegation_or_server_maximum() {
    let (_dir, store) = store();
    let delegation = store.issue_delegation("actor", "svc", &[]).unwrap();
    assert!(
        store
            .create_context(
                "svc",
                &delegation.delegation_token,
                "l",
                1,
                delegation.expires_at_unix_ms + 1,
            )
            .is_err()
    );
    assert!(
        store
            .delegation_actor("svc", &delegation.delegation_token)
            .is_ok()
    );
}

#[test]
fn canonical_digest_sorts_top_level_and_nested_object_keys() {
    let first: serde_json::Value =
        serde_json::from_str(r#"{"owner":"a","nested":{"repo":"b","labels":[{"z":1,"a":2}]}}"#)
            .unwrap();
    let reordered: serde_json::Value =
        serde_json::from_str(r#"{"nested":{"labels":[{"a":2,"z":1}],"repo":"b"},"owner":"a"}"#)
            .unwrap();
    assert_eq!(
        canonical_args_hash(&first).unwrap(),
        canonical_args_hash(&reordered).unwrap()
    );
}

#[test]
fn approval_is_fully_bound_and_single_use() {
    let (_dir, store) = store();
    let context = context(&store);
    let approval = store
        .issue_approval(
            &context.execution_context_id,
            "axon-service",
            "mcp:github::delete",
            "args-a",
            "contract-a",
        )
        .unwrap();
    assert!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "key-wrong",
                "mcp:github::delete",
                "args-b",
                "contract-a",
                Some(&approval.approval_token),
                true
            )
            .is_err()
    );
    assert!(matches!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "key-1",
                "mcp:github::delete",
                "args-a",
                "contract-a",
                Some(&approval.approval_token),
                true
            )
            .unwrap(),
        Reservation::Execute { .. }
    ));
    assert!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "key-2",
                "mcp:github::delete",
                "args-a",
                "contract-a",
                Some(&approval.approval_token),
                true
            )
            .is_err()
    );
}

#[test]
fn approval_context_rejects_a_different_actor() {
    let (_dir, store) = store();
    let context = context(&store);
    assert!(
        store
            .bound_context_for_actor(&context.execution_context_id, "forged-actor")
            .is_err()
    );
    assert_eq!(
        store
            .bound_context_for_actor(&context.execution_context_id, "actor-1")
            .unwrap()
            .service,
        "axon-service"
    );
}

#[test]
fn expired_and_forged_approvals_fail_closed() {
    let (_dir, store) = store();
    let context = context(&store);
    let approval = store
        .issue_approval(
            &context.execution_context_id,
            "axon-service",
            "mcp:github::delete",
            "args",
            "contract",
        )
        .unwrap();
    assert!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "forged",
                "mcp:github::delete",
                "args",
                "contract",
                Some("apr_forged"),
                true
            )
            .is_err()
    );
    store
        .conn()
        .unwrap()
        .execute("UPDATE agent_approvals SET expires_at=?1", [now_ms() - 1])
        .unwrap();
    assert!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "expired",
                "mcp:github::delete",
                "args",
                "contract",
                Some(&approval.approval_token),
                true
            )
            .is_err()
    );
}

#[test]
fn exact_replay_returns_original_receipt_and_mismatch_fails() {
    let (_dir, store) = store();
    let context = context(&store);
    assert!(matches!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "idem",
                "mcp:github::create",
                "args",
                "contract",
                None,
                false
            )
            .unwrap(),
        Reservation::Execute { .. }
    ));
    let original = store
        .finish(
            "idem",
            AgentExecutionStatus::Succeeded,
            Some(&serde_json::json!({"ok": true})),
            None,
        )
        .unwrap();
    let replay = store
        .reserve(
            &context.execution_context_id,
            "axon-service",
            "idem",
            "mcp:github::create",
            "args",
            "contract",
            None,
            false,
        )
        .unwrap();
    assert!(matches!(&replay, Reservation::Existing(_)));
    if let Reservation::Existing(replayed) = replay {
        assert_eq!(replayed, original);
    }
    assert!(
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "idem",
                "mcp:github::create",
                "different",
                "contract",
                None,
                false
            )
            .is_err()
    );
}

#[test]
fn concurrent_duplicate_has_one_dispatch_owner() {
    let (_dir, store) = store();
    let context = context(&store);
    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(8));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let barrier = barrier.clone();
        let context_id = context.execution_context_id.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            store
                .reserve(
                    &context_id,
                    "axon-service",
                    "same-key",
                    "mcp:github::create",
                    "args",
                    "contract",
                    None,
                    false,
                )
                .unwrap()
        }));
    }
    let outcomes = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| matches!(value, Reservation::Execute { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|value| matches!(value, Reservation::Running(_)))
            .count(),
        7
    );
}

#[test]
fn restart_marks_unknown_running_execution_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.sqlite3");
    {
        let store = AgentExecutionStore::open(path.clone()).unwrap();
        let context = context(&store);
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                "running",
                "mcp:github::create",
                "args",
                "contract",
                None,
                false,
            )
            .unwrap();
    }
    let reopened = AgentExecutionStore::open(path).unwrap();
    assert_eq!(
        reopened.status("running").unwrap().unwrap().status,
        AgentExecutionStatus::Interrupted
    );
}

#[test]
fn cancellation_and_timeout_are_durable_terminal_states() {
    let (_dir, store) = store();
    let context = context(&store);
    for (key, status, kind) in [
        ("cancel", AgentExecutionStatus::Cancelled, "cancelled"),
        (
            "timeout",
            AgentExecutionStatus::TimedOut,
            "deadline_exceeded",
        ),
    ] {
        store
            .reserve(
                &context.execution_context_id,
                "axon-service",
                key,
                "mcp:github::create",
                "args",
                "contract",
                None,
                false,
            )
            .unwrap();
        let receipt = store.finish(key, status, None, Some(kind)).unwrap();
        assert_eq!(receipt.status, status);
        assert_eq!(store.status(key).unwrap().unwrap(), receipt);
    }
}

#[test]
fn persisted_audit_is_correlated_and_redacted() {
    let (_dir, store) = store();
    let context = context(&store);
    let secret_params = serde_json::json!({"authorization": "Bearer super-secret"});
    let args_hash = canonical_args_hash(&secret_params).unwrap();
    store
        .reserve(
            &context.execution_context_id,
            "axon-service",
            "audit-key",
            "mcp:github::create",
            &args_hash,
            "contract",
            None,
            false,
        )
        .unwrap();
    let receipt = store
        .finish(
            "audit-key",
            AgentExecutionStatus::Succeeded,
            Some(&serde_json::json!({"token": "response-secret"})),
            None,
        )
        .unwrap();
    assert_eq!(receipt.actor, "actor-1");
    assert_eq!(receipt.service, "axon-service");
    assert_eq!(receipt.loadout_id, "loadout-1");
    let conn = store.conn().unwrap();
    let persisted: String = conn.query_row("SELECT actor || service || loadout_id || tool_id || args_hash || contract_hash || audit_id || receipt_id FROM agent_requests WHERE idempotency_key='audit-key'", [], |row| row.get(0)).unwrap();
    assert!(persisted.contains("actor-1axon-serviceloadout-1mcp:github::create"));
    assert!(!persisted.contains("super-secret"));
    let tokens: String = conn
        .query_row(
            "SELECT token_hash FROM agent_delegations LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(tokens.starts_with("sha256:"));
    assert!(!tokens.contains("dlg_"));
}

#[test]
fn result_cap_and_bounded_pruning_preserve_replay_window() {
    let (_dir, store) = store();
    let context = context(&store);
    store
        .reserve(
            &context.execution_context_id,
            "axon-service",
            "large",
            "mcp:github::create",
            "args",
            "contract",
            None,
            false,
        )
        .unwrap();
    let oversized = serde_json::json!({"data": "x".repeat(MAX_RESULT_BYTES + 1)});
    let receipt = store
        .finish(
            "large",
            AgentExecutionStatus::Succeeded,
            Some(&oversized),
            None,
        )
        .unwrap();
    assert_eq!(receipt.status, AgentExecutionStatus::Failed);
    assert_eq!(receipt.error_kind.as_deref(), Some("result_too_large"));
    assert!(receipt.result.is_none());
    assert_eq!(
        store.prune_expired().unwrap(),
        0,
        "fresh replay is retained"
    );
    store
        .conn()
        .unwrap()
        .execute(
            "UPDATE agent_requests SET updated_at=?1 WHERE idempotency_key='large'",
            [now_ms() - REPLAY_RETENTION_MS - 1],
        )
        .unwrap();
    assert_eq!(store.prune_expired().unwrap(), 1);
    assert!(store.status("large").unwrap().is_none());
}

#[test]
fn operational_reads_prune_expired_rows_without_new_delegation() {
    let (_dir, store) = store();
    store.issue_delegation("actor", "service", &[]).unwrap();
    store
        .conn()
        .unwrap()
        .execute("UPDATE agent_delegations SET expires_at=?1", [now_ms() - 1])
        .unwrap();
    store.last_prune_at.store(0, Ordering::Release);

    assert!(store.status("unrelated-status-poll").unwrap().is_none());
    let remaining: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM agent_delegations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn migration_is_versioned_transactional_and_accepts_only_duplicate_column() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");
    {
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    let migrated = AgentExecutionStore::open(path.clone()).unwrap();
    let version: i64 = migrated
        .conn()
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, AGENT_SCHEMA_VERSION);
    drop(migrated);

    let broken_path = dir.path().join("broken.sqlite3");
    let connection = Connection::open(&broken_path).unwrap();
    connection
        .execute_batch("CREATE TABLE agent_contexts(wrong TEXT); PRAGMA user_version=0;")
        .unwrap();
    drop(connection);
    let error = AgentExecutionStore::open(broken_path).err().unwrap();
    assert!(error.to_string().contains("agent execution storage failed"));
    let connection = Connection::open(dir.path().join("broken.sqlite3")).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0, "failed migration must roll back its version");
}
