//! Exact publication-bound regular Tool calls.
//!
//! This crate-private kernel is deliberately unmounted and is freshness-only:
//! it does not authorize AssetUse/admin/destructive execution or perform
//! elicitation. Usage telemetry is scheduled only after exact checked apply and
//! remains unattributed for this regular pool path. SEP-2243 header recovery is
//! omitted because its peer-cache mutation is not yet incarnation-safe.
#![allow(dead_code)]
#![allow(
    clippy::items_after_test_module,
    reason = "the focused exact-kernel fixtures stay adjacent to the public error contract"
)]

use std::time::Instant;

use rmcp::model::{CallToolRequestParams, CallToolResponse, ErrorData};

use super::capability_call::bound_upstream_service_error;
use super::helpers::{estimate_call_tool_response_size, max_response_bytes};
use super::logging::UpstreamRequestLog;
use super::usage_record::{record_usage_call, record_usage_call_with_response};
use super::{ToolCatalogGeneration, UpstreamPool};
use crate::upstream::types::UpstreamCapability;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExactToolCallError {
    #[error("published tool target is unavailable")]
    Unavailable,
    #[error("published tool call queue is unavailable")]
    QueueUnavailable,
    #[error("upstream tool returned an MCP error")]
    Mcp(ErrorData),
    #[error("upstream tool transport failed")]
    Transport,
    #[error("upstream tool protocol failed")]
    Protocol,
    #[error("upstream tool call timed out")]
    Timeout,
    #[error("upstream tool call was cancelled")]
    Cancelled,
    #[error("upstream tool input-required rounds were exceeded")]
    InputRequiredRoundsExceeded,
    #[error("upstream tool call failed")]
    Other,
    #[error("upstream tool response is too large")]
    TooLarge,
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::panic,
    reason = "exact-kernel tests construct upstream-owned Tool descriptors directly"
)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CreateTaskResult,
        ElicitRequest, ElicitRequestParams, ElicitationSchema, ErrorData, InputRequest,
        InputRequests, InputRequiredResult, ListToolsResult, PaginatedRequestParams,
        PrimitiveSchemaDefinition, RequestMetaObject, Task, TaskStatus, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::{ExactToolCallError, PreparedExactToolCall, PreparedOutcome, UpstreamPool};
    use crate::upstream::pool::testsupport::catalog_pool_with_server;
    use crate::upstream::types::UpstreamTool;

    #[derive(Clone)]
    struct InspectingToolServer {
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(CallToolRequestParams, RequestMetaObject)>>>,
        error: Option<ErrorData>,
    }

    impl ServerHandler for InspectingToolServer {
        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received.lock().await.push((request, context.meta));
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(CallToolResult::success(vec![ContentBlock::text("exact")]).into())
        }
    }

    #[derive(Clone)]
    struct DelayedToolServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedToolServer {
        async fn call_tool(
            &self,
            _: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                Err(ErrorData::internal_error(
                    "private delayed tool failure",
                    None,
                ))
            } else {
                Ok(CallToolResult::success(vec![ContentBlock::text("delayed")]).into())
            }
        }
    }

    #[derive(Clone)]
    struct SlowToolServer {
        calls: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl ServerHandler for SlowToolServer {
        async fn call_tool(
            &self,
            _: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(CallToolResult::success(vec![ContentBlock::text("slow")]).into())
        }
    }

    #[derive(Clone)]
    struct HeaderMismatchToolServer {
        calls: Arc<AtomicUsize>,
        lists: Arc<AtomicUsize>,
    }

    impl ServerHandler for HeaderMismatchToolServer {
        async fn list_tools(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            self.lists.fetch_add(1, Ordering::SeqCst);
            Ok(ListToolsResult::with_all_items(Vec::new()))
        }

        async fn call_tool(
            &self,
            _: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ErrorData::new(
                rmcp::model::ErrorCode::HEADER_MISMATCH,
                "stale headers",
                None,
            ))
        }
    }

    #[derive(Clone)]
    struct SizedToolServer(usize);

    impl ServerHandler for SizedToolServer {
        async fn call_tool(
            &self,
            _: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            Ok(CallToolResult::success(vec![ContentBlock::text("x".repeat(self.0))]).into())
        }
    }

    async fn set_tool(pool: &UpstreamPool, description: &str, destructive: bool) {
        let tool = Tool::new(
            "nested/tool",
            description.to_string(),
            Arc::new(serde_json::Map::new()),
        );
        let upstream_name: Arc<str> = Arc::from("alpha");
        let upstream_tool = UpstreamTool {
            input_schema: Some(serde_json::Value::Object((*tool.input_schema).clone())),
            output_schema: None,
            destructive,
            upstream_name,
            tool,
        };
        let mut catalog = pool.catalog_write().await;
        catalog
            .get_mut("alpha")
            .unwrap()
            .tools
            .insert("nested/tool".into(), upstream_tool);
    }

    async fn fixture(
        error: Option<ErrorData>,
        destructive: bool,
    ) -> (
        Arc<UpstreamPool>,
        Arc<AtomicUsize>,
        Arc<Mutex<Vec<(CallToolRequestParams, RequestMetaObject)>>>,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingToolServer {
                calls: Arc::clone(&calls),
                received: Arc::clone(&received),
                error,
            },
        )
        .await;
        set_tool(&pool, "exact", destructive).await;
        (pool, calls, received)
    }

    async fn wait_for_usage_count(store: &crate::usage::UsageStore, outcome: &str) -> i64 {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let expected = outcome.to_string();
            let count = store
                .with_conn(move |connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM upstream_calls WHERE upstream_name = 'alpha' AND tool_name = 'nested/tool' AND subject_scoped = 0 AND actor = 'unattributed' AND outcome = ?1",
                            [expected],
                            |row| row.get(0),
                        )
                        .map_err(crate::usage::store::sqlite_error)
                })
                .await
                .unwrap();
            if count > 0 || std::time::Instant::now() >= deadline {
                return count;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn with_usage_store(
        pool: Arc<UpstreamPool>,
        store: &Arc<crate::usage::UsageStore>,
    ) -> Arc<UpstreamPool> {
        let Some(pool) = Arc::into_inner(pool) else {
            panic!("fixture pool has one owner")
        };
        Arc::new(pool.with_usage_store(Some(Arc::clone(store))))
    }

    async fn total_usage_count(store: &crate::usage::UsageStore) -> i64 {
        store
            .with_conn(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn exact_tool_kernel_records_usage_only_after_current_checked_apply() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("usage.db"))
                .await
                .unwrap(),
        );
        for (error, outcome) in [
            (None, "ok"),
            (
                Some(ErrorData::internal_error("private", None)),
                "upstream_error",
            ),
        ] {
            let (pool, _, _) = fixture(error, false).await;
            let pool = with_usage_store(pool, &store);
            let generation = pool.published_tool_catalog().await.unwrap().generation();
            let _result = pool
                .call_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await;
            assert_eq!(wait_for_usage_count(&store, outcome).await, 1);
            if outcome == "ok" {
                let row: (String, String, String, i64, i64) = store
                    .with_conn(|connection| {
                        connection
                            .query_row(
                                "SELECT capability, operation, actor, elapsed_ms, response_bytes FROM upstream_calls WHERE outcome = 'ok'",
                                [],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                            )
                            .map_err(crate::usage::store::sqlite_error)
                    })
                    .await
                    .unwrap();
                assert_eq!(
                    (row.0.as_str(), row.1.as_str(), row.2.as_str()),
                    ("tools", "tool.call", "unattributed")
                );
                assert!(row.3 >= 0);
                assert!(row.4 > 0);
            }
        }
    }

    #[tokio::test]
    async fn stale_prepared_tool_outcome_records_no_usage() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("usage.db"))
                .await
                .unwrap(),
        );
        let (pool, _, _) = fixture(None, false).await;
        let pool = with_usage_store(pool, &store);
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let prepared = pool
            .prepare_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool"),
            )
            .await
            .unwrap();
        set_tool(&pool, "replacement", false).await;
        assert!(matches!(
            pool.apply_prepared_tool_exact(prepared).await,
            Err(ExactToolCallError::Unavailable)
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(total_usage_count(&store).await, 0);
    }

    #[tokio::test]
    async fn exact_tool_usage_records_too_large_and_timeout_but_not_queue_or_abort() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("usage.db"))
                .await
                .unwrap(),
        );

        let pool = catalog_pool_with_server("alpha", SizedToolServer(12 * 1024 * 1024)).await;
        let pool = with_usage_store(pool, &store);
        set_tool(&pool, "large", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool")
            )
            .await,
            Err(ExactToolCallError::TooLarge)
        ));
        assert_eq!(wait_for_usage_count(&store, "response_too_large").await, 1);
        let bytes: i64 = store
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT response_bytes FROM upstream_calls WHERE outcome = 'response_too_large'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap();
        assert!(bytes > 10 * 1024 * 1024);

        let timeout_store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("timeout.db"))
                .await
                .unwrap(),
        );
        let pool = catalog_pool_with_server(
            "alpha",
            SlowToolServer {
                calls: Arc::new(AtomicUsize::new(0)),
                delay: Duration::from_millis(80),
            },
        )
        .await;
        let Some(mut pool) = Arc::into_inner(pool) else {
            panic!("fixture pool has one owner")
        };
        pool.request_timeout = Duration::from_millis(25);
        let pool = Arc::new(pool.with_usage_store(Some(Arc::clone(&timeout_store))));
        set_tool(&pool, "slow", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool")
            )
            .await,
            Err(ExactToolCallError::Timeout)
        ));
        assert_eq!(wait_for_usage_count(&timeout_store, "timeout").await, 1);

        let queue_store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("queue.db"))
                .await
                .unwrap(),
        );
        let pool = catalog_pool_with_server(
            "alpha",
            SlowToolServer {
                calls: Arc::new(AtomicUsize::new(0)),
                delay: Duration::from_millis(1),
            },
        )
        .await;
        let Some(mut pool) = Arc::into_inner(pool) else {
            panic!("fixture pool has one owner")
        };
        pool.request_timeout = Duration::from_millis(20);
        pool.call_concurrency = 1;
        pool.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        let pool = Arc::new(pool.with_usage_store(Some(Arc::clone(&queue_store))));
        set_tool(&pool, "queue", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let _held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool")
            )
            .await,
            Err(ExactToolCallError::QueueUnavailable)
        ));
        assert_eq!(total_usage_count(&queue_store).await, 0);

        let abort_store = Arc::new(
            crate::usage::UsageStore::open(directory.path().join("abort.db"))
                .await
                .unwrap(),
        );
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedToolServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        let pool = with_usage_store(pool, &abort_store);
        set_tool(&pool, "abort", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .call_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await
        });
        started.notified().await;
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(total_usage_count(&abort_store).await, 0);
    }

    #[tokio::test]
    async fn exact_tool_kernel_forwards_native_params_and_preserves_complete_response() {
        let (pool, calls, received) = fixture(None, true).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let mut meta = RequestMetaObject::new();
        meta.insert("trace".into(), serde_json::json!("opaque"));
        let mut params =
            CallToolRequestParams::new("nested/tool").with_arguments(serde_json::Map::from_iter([
                ("value".into(), serde_json::json!(7)),
            ]));
        params.meta = Some(meta.clone());
        let response = pool
            .call_published_tool_exact("alpha", "nested/tool", generation, params)
            .await
            .expect("fresh exact destructive Tool route is callable by freshness kernel");
        assert!(matches!(response, CallToolResponse::Complete(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let received = received.lock().await;
        assert_eq!(received[0].0.name, "nested/tool");
        assert_eq!(received[0].0.arguments.as_ref().unwrap()["value"], 7);
        assert_eq!(received[0].1.get("trace"), meta.get("trace"));
    }

    #[tokio::test]
    async fn exact_tool_kernel_rejects_wrong_name_and_stale_generation_without_rpc() {
        let (pool, calls, _) = fixture(None, false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("other")
            )
            .await,
            Err(ExactToolCallError::Unavailable)
        ));
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().tools.clear();
        }
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool")
            )
            .await,
            Err(ExactToolCallError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_tool_kernel_preserves_bounded_mcp_error_and_records_success() {
        let private = "private".repeat(1000);
        let (pool, _, _) = fixture(
            Some(ErrorData::invalid_params(
                private,
                Some(serde_json::json!({"kind": "invalid_arguments"})),
            )),
            false,
        )
        .await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let error = pool
            .call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool"),
            )
            .await
            .expect_err("MCP error is preserved");
        let ExactToolCallError::Mcp(data) = error else {
            panic!("expected structured MCP error")
        };
        assert_eq!(data.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(data.message.len() < 7_000);
        assert!(data.message.contains("truncated"));
        assert_eq!(pool.upstream_tool_last_error("alpha").await, None);
    }

    #[tokio::test]
    async fn exact_tool_kernel_preserves_non_complete_responses_through_checked_apply() {
        let (pool, _, _) = fixture(None, false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::default()),
            )
            .build()
            .unwrap();
        let input_required = InputRequiredResult::from_input_requests(InputRequests::from([(
            "confirmation".into(),
            InputRequest::Elicitation(ElicitRequest::new(
                ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: "confirm?".into(),
                    requested_schema: schema,
                },
            )),
        )]));
        let responses = [
            CallToolResponse::Task(CreateTaskResult::new(Task::new(
                "task-7",
                TaskStatus::Working,
                "2026-08-24T00:00:00Z",
                "2026-08-24T00:00:00Z",
            ))),
            CallToolResponse::InputRequired(input_required),
        ];

        // The generic pool stores a unit client handler, so rmcp capability
        // negotiation rejects these variants before this kernel. Once a future
        // capable mount accepts them, checked apply preserves the exact value.
        for expected in responses {
            {
                let mut catalog = pool.catalog_write().await;
                catalog.get_mut("alpha").unwrap().tool_last_error = Some("sentinel".into());
            }
            let observed = pool
                .observe_tool_call("alpha", "nested/tool", generation)
                .await
                .unwrap();
            let actual = pool
                .apply_prepared_tool_exact(PreparedExactToolCall {
                    observed,
                    generation,
                    native_name: "nested/tool".into(),
                    outcome: PreparedOutcome::Response(expected.clone()),
                    elapsed_ms: 1,
                })
                .await
                .unwrap();
            match (actual, expected) {
                (CallToolResponse::Task(actual), CallToolResponse::Task(expected)) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                (
                    CallToolResponse::InputRequired(actual),
                    CallToolResponse::InputRequired(expected),
                ) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                _ => panic!("checked apply changed the CallToolResponse variant"),
            }
            assert_eq!(pool.upstream_tool_last_error("alpha").await, None);
        }
    }

    #[tokio::test]
    async fn exact_tool_kernel_discards_tool_publication_aba_without_health_mutation() {
        for error in [None, Some(ErrorData::internal_error("private", None))] {
            let (pool, _, _) = fixture(error, false).await;
            let generation = pool.published_tool_catalog().await.unwrap().generation();
            let prepared = pool
                .prepare_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await
                .unwrap();
            {
                let mut catalog = pool.catalog_write().await;
                catalog.get_mut("alpha").unwrap().tool_last_error = Some("sentinel".into());
            }
            set_tool(&pool, "replacement", false).await;
            set_tool(&pool, "exact", false).await;
            assert_ne!(
                pool.published_tool_catalog().await.unwrap().generation(),
                generation
            );
            assert!(matches!(
                pool.apply_prepared_tool_exact(prepared).await,
                Err(ExactToolCallError::Unavailable)
            ));
            assert_eq!(
                pool.upstream_tool_last_error("alpha").await.as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn exact_tool_kernel_discards_connection_aba_success_and_error() {
        for fail in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(
                crate::usage::UsageStore::open(directory.path().join("usage.db"))
                    .await
                    .unwrap(),
            );
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let delayed = DelayedToolServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail,
            };
            let pool = catalog_pool_with_server("alpha", delayed.clone()).await;
            let pool = with_usage_store(pool, &store);
            set_tool(&pool, "exact", false).await;
            let generation = pool.published_tool_catalog().await.unwrap().generation();
            let task_pool = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                task_pool
                    .call_published_tool_exact(
                        "alpha",
                        "nested/tool",
                        generation,
                        CallToolRequestParams::new("nested/tool"),
                    )
                    .await
            });
            started.notified().await;
            let replacement = catalog_pool_with_server(
                "alpha",
                InspectingToolServer {
                    calls: Arc::new(AtomicUsize::new(0)),
                    received: Arc::new(Mutex::new(Vec::new())),
                    error: None,
                },
            )
            .await;
            let (connection_b, entry_b) =
                replacement.remove_connection_catalog_entry("alpha").await;
            let previous_a = pool
                .install_connection_catalog_entry(
                    "alpha".into(),
                    connection_b.unwrap(),
                    entry_b.unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            let (removed_b, _) = pool.remove_connection_catalog_entry("alpha").await;
            let mut entry_a =
                super::super::entries::healthy_in_process_entry(Arc::from("alpha"), HashMap::new());
            entry_a.tool_last_error = Some("sentinel".into());
            pool.install_connection_catalog_entry("alpha".into(), previous_a, entry_a)
                .await
                .unwrap();
            set_tool(&pool, "exact", false).await;
            release.notify_one();
            assert!(matches!(
                task.await.unwrap(),
                Err(ExactToolCallError::Unavailable)
            ));
            assert_eq!(
                pool.upstream_tool_last_error("alpha").await.as_deref(),
                Some("sentinel")
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(total_usage_count(&store).await, 0);
            if let Some(connection) = removed_b {
                connection.shutdown("alpha", "test.tool-call.aba").await;
            }
        }
    }

    #[tokio::test]
    async fn exact_tool_kernel_uses_one_queue_and_rpc_deadline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowToolServer {
                calls: Arc::clone(&calls),
                delay: Duration::from_millis(80),
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).unwrap();
        pool_mut.request_timeout = Duration::from_millis(100);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        set_tool(&pool, "slow", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .call_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(70)).await;
        drop(held);
        assert!(matches!(
            task.await.unwrap(),
            Err(ExactToolCallError::Timeout)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exact_tool_kernel_queue_saturation_does_not_call_or_mutate_health() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowToolServer {
                calls: Arc::clone(&calls),
                delay: Duration::from_millis(1),
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).unwrap();
        pool_mut.request_timeout = Duration::from_millis(25);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        set_tool(&pool, "slow", false).await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().tool_last_error = Some("sentinel".into());
        }
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let _held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        assert!(matches!(
            pool.call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool"),
            )
            .await,
            Err(ExactToolCallError::QueueUnavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_tool_kernel_does_not_retry_header_mismatch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let lists = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            HeaderMismatchToolServer {
                calls: Arc::clone(&calls),
                lists: Arc::clone(&lists),
            },
        )
        .await;
        set_tool(&pool, "header", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        let error = pool
            .call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool"),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, ExactToolCallError::Mcp(data) if data.code == rmcp::model::ErrorCode::HEADER_MISMATCH)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(lists.load(Ordering::SeqCst), 0);
        let metrics = pool.header_recovery_metrics("alpha");
        assert_eq!(metrics.mismatch_detected, 0);
        assert_eq!(metrics.schema_refreshes, 0);
        assert_eq!(metrics.retry_successes, 0);
        assert_eq!(metrics.retry_failures, 0);
    }

    #[tokio::test]
    async fn exact_tool_kernel_cancellation_releases_permit_without_health_mutation() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedToolServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        set_tool(&pool, "cancel", false).await;
        let generation = pool.published_tool_catalog().await.unwrap().generation();
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().tool_last_error = Some("sentinel".into());
        }
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .call_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await
        });
        started.notified().await;
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("sentinel")
        );
        let _permit = tokio::time::timeout(
            Duration::from_millis(50),
            pool.acquire_upstream_call_permit("alpha"),
        )
        .await
        .expect("aborting exact Tool call releases permit")
        .unwrap();
    }

    #[tokio::test]
    async fn exact_tool_kernel_enforces_serialized_response_byte_boundary_as_healthy() {
        let calibration = catalog_pool_with_server("alpha", SizedToolServer(0)).await;
        set_tool(&calibration, "calibration", false).await;
        let generation = calibration
            .published_tool_catalog()
            .await
            .unwrap()
            .generation();
        let empty = calibration
            .call_published_tool_exact(
                "alpha",
                "nested/tool",
                generation,
                CallToolRequestParams::new("nested/tool"),
            )
            .await
            .unwrap();
        let overhead = super::super::helpers::estimate_call_tool_response_size(&empty);
        let exact_payload = super::super::helpers::max_response_bytes()
            .checked_sub(overhead)
            .expect("configured cap exceeds empty Tool response");
        for (extra, too_large) in [(0, false), (1, true)] {
            let pool =
                catalog_pool_with_server("alpha", SizedToolServer(exact_payload + extra)).await;
            set_tool(&pool, "large", false).await;
            let generation = pool.published_tool_catalog().await.unwrap().generation();
            let result = pool
                .call_published_tool_exact(
                    "alpha",
                    "nested/tool",
                    generation,
                    CallToolRequestParams::new("nested/tool"),
                )
                .await;
            assert_eq!(
                matches!(result, Err(ExactToolCallError::TooLarge)),
                too_large
            );
            assert_eq!(pool.upstream_tool_last_error("alpha").await, None);
        }
    }
}

enum PreparedOutcome {
    Response(CallToolResponse),
    Mcp(ErrorData),
    Transport,
    Protocol,
    Timeout,
    Cancelled,
    InputRequiredRoundsExceeded,
    Other,
}

pub(crate) struct PreparedExactToolCall {
    observed: super::incarnation::ObservedConnectionCatalogEntry,
    generation: ToolCatalogGeneration,
    native_name: String,
    outcome: PreparedOutcome,
    elapsed_ms: u128,
}

impl UpstreamPool {
    pub(crate) async fn call_published_tool_exact(
        &self,
        upstream_name: &str,
        native_name: &str,
        generation: ToolCatalogGeneration,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, ExactToolCallError> {
        let prepared = self
            .prepare_published_tool_exact(upstream_name, native_name, generation, params)
            .await?;
        self.apply_prepared_tool_exact(prepared).await
    }

    pub(crate) async fn prepare_published_tool_exact(
        &self,
        upstream_name: &str,
        native_name: &str,
        generation: ToolCatalogGeneration,
        params: CallToolRequestParams,
    ) -> Result<PreparedExactToolCall, ExactToolCallError> {
        if params.name != native_name {
            return Err(ExactToolCallError::Unavailable);
        }
        let start = Instant::now();
        let permit = tokio::time::timeout(
            self.request_timeout,
            self.acquire_upstream_call_permit(upstream_name),
        )
        .await;
        let _permit = match permit {
            Ok(Ok(permit)) => permit,
            _ => return Err(ExactToolCallError::QueueUnavailable),
        };
        let Some(observed) = self
            .observe_tool_call(upstream_name, native_name, generation)
            .await
        else {
            return Err(ExactToolCallError::Unavailable);
        };
        let remaining = self.request_timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(ExactToolCallError::QueueUnavailable);
        }
        let outcome =
            match tokio::time::timeout(remaining, observed.peer.call_tool_once(params)).await {
                Err(_) => PreparedOutcome::Timeout,
                Ok(Ok(response)) => PreparedOutcome::Response(response),
                Ok(Err(error)) => match bound_upstream_service_error(error) {
                    rmcp::ServiceError::McpError(data) => PreparedOutcome::Mcp(data),
                    rmcp::ServiceError::Timeout { .. } => PreparedOutcome::Timeout,
                    rmcp::ServiceError::TransportSend(_)
                    | rmcp::ServiceError::TransportClosed
                    | rmcp::ServiceError::SubscriptionLagged { .. } => PreparedOutcome::Transport,
                    rmcp::ServiceError::UnexpectedResponse => PreparedOutcome::Protocol,
                    rmcp::ServiceError::Cancelled { .. } => PreparedOutcome::Cancelled,
                    rmcp::ServiceError::InputRequiredRoundsExceeded { .. } => {
                        PreparedOutcome::InputRequiredRoundsExceeded
                    }
                    _ => PreparedOutcome::Other,
                },
            };
        Ok(PreparedExactToolCall {
            observed,
            generation,
            native_name: native_name.to_string(),
            outcome,
            elapsed_ms: start.elapsed().as_millis(),
        })
    }

    pub(crate) async fn apply_prepared_tool_exact(
        &self,
        prepared: PreparedExactToolCall,
    ) -> Result<CallToolResponse, ExactToolCallError> {
        let upstream = prepared.observed.upstream().to_string();
        match prepared.outcome {
            PreparedOutcome::Response(response) => {
                let response_bytes = estimate_call_tool_response_size(&response);
                let too_large = response_bytes > max_response_bytes();
                let elapsed_ms = prepared.elapsed_ms;
                let applied = self
                    .apply_to_observed_tool_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            super::health::record_success_on_entry(
                                &upstream,
                                entry,
                                UpstreamCapability::Tools,
                            );
                            record_usage_call_with_response(
                                self,
                                UpstreamRequestLog::tool(&upstream, &prepared.native_name, false),
                                None,
                                if too_large {
                                    "response_too_large"
                                } else {
                                    "ok"
                                },
                                elapsed_ms,
                                Some(response_bytes),
                            );
                        },
                    )
                    .await;
                if applied.is_none() {
                    Err(ExactToolCallError::Unavailable)
                } else if too_large {
                    Err(ExactToolCallError::TooLarge)
                } else {
                    Ok(response)
                }
            }
            PreparedOutcome::Mcp(data) => {
                let elapsed_ms = prepared.elapsed_ms;
                let applied = self
                    .apply_to_observed_tool_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            super::health::record_success_on_entry(
                                &upstream,
                                entry,
                                UpstreamCapability::Tools,
                            );
                            record_usage_call(
                                self,
                                UpstreamRequestLog::tool(&upstream, &prepared.native_name, false),
                                None,
                                "upstream_error",
                                elapsed_ms,
                            );
                        },
                    )
                    .await;
                applied
                    .map(|()| ExactToolCallError::Mcp(data))
                    .map_or(Err(ExactToolCallError::Unavailable), Err)
            }
            outcome @ (PreparedOutcome::Cancelled
            | PreparedOutcome::InputRequiredRoundsExceeded) => {
                let error = if matches!(outcome, PreparedOutcome::Cancelled) {
                    ExactToolCallError::Cancelled
                } else {
                    ExactToolCallError::InputRequiredRoundsExceeded
                };
                let elapsed_ms = prepared.elapsed_ms;
                self.apply_to_observed_tool_call(
                    &prepared.observed,
                    prepared.generation,
                    &prepared.native_name,
                    |_| {
                        record_usage_call(
                            self,
                            UpstreamRequestLog::tool(&upstream, &prepared.native_name, false),
                            None,
                            "upstream_error",
                            elapsed_ms,
                        );
                    },
                )
                .await
                .map(|()| error)
                .map_or(Err(ExactToolCallError::Unavailable), Err)
            }
            outcome @ (PreparedOutcome::Transport
            | PreparedOutcome::Protocol
            | PreparedOutcome::Timeout
            | PreparedOutcome::Other) => {
                let (error, reason) = match outcome {
                    PreparedOutcome::Transport => (
                        ExactToolCallError::Transport,
                        "upstream tool transport failed",
                    ),
                    PreparedOutcome::Protocol => (
                        ExactToolCallError::Protocol,
                        "upstream tool protocol failed",
                    ),
                    PreparedOutcome::Timeout => {
                        (ExactToolCallError::Timeout, "upstream tool call timed out")
                    }
                    PreparedOutcome::Other => {
                        (ExactToolCallError::Other, "upstream tool call failed")
                    }
                    _ => (ExactToolCallError::Other, "upstream tool call failed"),
                };
                let usage_outcome = if matches!(outcome, PreparedOutcome::Timeout) {
                    "timeout"
                } else {
                    "upstream_error"
                };
                let elapsed_ms = prepared.elapsed_ms;
                let applied = self
                    .apply_to_observed_tool_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            super::health::record_failure_on_entry(
                                &upstream,
                                entry,
                                UpstreamCapability::Tools,
                                reason.to_string(),
                            );
                            record_usage_call(
                                self,
                                UpstreamRequestLog::tool(&upstream, &prepared.native_name, false),
                                None,
                                usage_outcome,
                                elapsed_ms,
                            );
                        },
                    )
                    .await;
                if applied.is_none() {
                    Err(ExactToolCallError::Unavailable)
                } else {
                    Err(error)
                }
            }
        }
    }
}
