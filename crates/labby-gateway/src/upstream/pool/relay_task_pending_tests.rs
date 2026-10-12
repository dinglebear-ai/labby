//! Bounded early task-status retention, through route and ordered relay paths.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    ClientCapabilities, ClientInfo, DetailedTask, GetMeta, JsonRpcMessage, NumberOrString,
    ServerInfo, ServerNotification, Task, TaskPayload, TaskStatus, TaskStatusNotification,
    TaskStatusNotificationParams,
};
use rmcp::service::{NotificationContext, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::{ClientHandler, RoleClient, ServerHandler, ServiceExt};
use tokio::sync::{Mutex, Notify};

use super::super::connect::OrderedRelayNotificationTransport;
use super::{RelayClientHandler, RelayRouteState, RelayTaskRouteState};

use super::task_pending::PendingTaskLimits as TestPendingLimits;

fn small_limits() -> TestPendingLimits {
    TestPendingLimits {
        ids: 2,
        per_id: 2,
        count: 3,
        bytes: 4_096,
        ttl: Duration::from_secs(1),
    }
}

fn route_state(limits: TestPendingLimits) -> RelayRouteState {
    RelayRouteState {
        tasks: Mutex::new(RelayTaskRouteState {
            test_pending_limits: Some(limits),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn limits(routes: &RelayRouteState) -> TestPendingLimits {
    routes
        .tasks
        .lock()
        .await
        .test_pending_limits
        .expect("test limits")
}

fn status(id: &str, message: &str) -> TaskStatusNotificationParams {
    TaskStatusNotificationParams::new(DetailedTask::new(
        Task::new(
            id,
            TaskStatus::Working,
            "2026-08-01T00:00:00Z",
            "2026-08-01T00:00:01Z",
        )
        .with_status_message(message),
        TaskPayload::Working,
    ))
}

async fn drain(
    routes: &RelayRouteState,
    ids: &[&str],
) -> BTreeMap<String, Vec<TaskStatusNotificationParams>> {
    let mut retained = BTreeMap::new();
    for id in ids {
        let notifications = routes.register_task_id(id, id).await;
        if !notifications.is_empty() {
            retained.insert((*id).to_owned(), notifications);
        }
    }
    retained
}

#[tokio::test]
async fn pending_task_repeated_id_retains_only_the_admitted_prefix() {
    let routes = route_state(small_limits());
    for message in ["first", "second", "third", "fourth"] {
        assert!(
            routes
                .translate_or_queue_task_status(status("native", message))
                .await
                .is_none()
        );
    }
    let cap = limits(&routes).await.per_id;
    let retained = routes.register_task_id("native", "gateway").await;
    assert_eq!(retained.len(), cap);
    assert_eq!(
        retained[0].task.task.status_message.as_deref(),
        Some("first")
    );
    assert_eq!(
        retained[1].task.task.status_message.as_deref(),
        Some("second")
    );
    assert!(
        retained
            .iter()
            .all(|event| event.task.task.task_id == "gateway")
    );
    assert!(
        routes
            .register_task_id("native", "gateway")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn pending_task_unknown_id_cardinality_is_bounded() {
    let routes = route_state(small_limits());
    let ids = ["one", "two", "three", "four"];
    for id in ids {
        assert!(
            routes
                .translate_or_queue_task_status(status(id, "early"))
                .await
                .is_none()
        );
    }
    let cap = limits(&routes).await.ids;
    assert!(drain(&routes, &ids).await.len() <= cap);
}

#[tokio::test]
async fn pending_task_total_count_is_bounded_independently_of_per_id_capacity() {
    let ids = ["one", "two", "three", "four"];
    let routes = route_state(TestPendingLimits {
        ids: 4,
        per_id: 4,
        ..small_limits()
    });
    for id in ids {
        for _ in 0..2 {
            assert!(
                routes
                    .translate_or_queue_task_status(status(id, "early"))
                    .await
                    .is_none()
            );
        }
    }
    let cap = limits(&routes).await.count;
    let retained = drain(&routes, &ids).await;
    assert!(retained.values().map(Vec::len).sum::<usize>() <= cap);
}

#[tokio::test]
async fn pending_task_serialized_bytes_include_escaped_utf8_payloads_and_keys() {
    let routes = route_state(TestPendingLimits {
        bytes: 512,
        ..small_limits()
    });
    let ids = ["one", "two"];
    let message = "é\n\"".repeat(32);
    for id in ids {
        assert!(
            routes
                .translate_or_queue_task_status(status(id, &message))
                .await
                .is_none()
        );
    }
    let cap = limits(&routes).await.bytes;
    let retained = drain(&routes, &ids).await;
    assert!(
        !retained.is_empty(),
        "byte admission must preserve a fitting event"
    );
    assert!(serde_json::to_vec(&retained).unwrap().len() <= cap);
}

#[tokio::test]
async fn pending_task_oversized_event_and_id_do_not_consume_admission() {
    let routes = route_state(TestPendingLimits {
        bytes: 512,
        ..small_limits()
    });
    let large_id = "large-id-".repeat(256);
    for params in [
        status("oversized", &"x".repeat(2_048)),
        status(&large_id, "early"),
    ] {
        assert!(
            routes
                .translate_or_queue_task_status(params)
                .await
                .is_none()
        );
    }
    assert!(
        routes
            .register_task_id("oversized", "oversized")
            .await
            .is_empty()
    );
    assert!(
        routes
            .register_task_id(&large_id, &large_id)
            .await
            .is_empty()
    );
    assert!(
        routes
            .translate_or_queue_task_status(status("fitting", "early"))
            .await
            .is_none()
    );
    assert_eq!(routes.register_task_id("fitting", "fitting").await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn pending_task_expiry_releases_capacity_without_delivering_stale_events() {
    let routes = route_state(TestPendingLimits {
        ids: 1,
        count: 1,
        ..small_limits()
    });
    assert!(
        routes
            .translate_or_queue_task_status(status("expired", "old"))
            .await
            .is_none()
    );
    let ttl = limits(&routes).await.ttl;
    tokio::time::advance(ttl + Duration::from_secs(1)).await;
    assert!(
        routes
            .translate_or_queue_task_status(status("fresh", "new"))
            .await
            .is_none()
    );
    assert!(
        routes
            .register_task_id("expired", "expired")
            .await
            .is_empty()
    );
    let retained = routes.register_task_id("fresh", "fresh").await;
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].task.task.status_message.as_deref(), Some("new"));
}

#[tokio::test]
async fn pending_task_registration_drain_frees_budget_and_mapped_events_bypass_queue_caps() {
    let routes = route_state(TestPendingLimits {
        ids: 1,
        count: 1,
        ..small_limits()
    });
    assert!(
        routes
            .translate_or_queue_task_status(status("first", "early"))
            .await
            .is_none()
    );
    assert_eq!(
        routes
            .register_task_id("first", "gateway-first")
            .await
            .len(),
        1
    );
    assert!(
        routes
            .translate_or_queue_task_status(status("second", "early"))
            .await
            .is_none()
    );
    let mapped = routes
        .translate_or_queue_task_status(status("first", "latest"))
        .await
        .unwrap();
    assert_eq!(mapped.task.task.task_id, "gateway-first");
    assert_eq!(
        routes
            .register_task_id("second", "gateway-second")
            .await
            .len(),
        1
    );
}

struct OrderedFixture {
    messages: VecDeque<RxJsonRpcMessage<RoleClient>>,
}

impl Transport<RoleClient> for OrderedFixture {
    type Error = Infallible;

    fn send(
        &mut self,
        _item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(Ok(()))
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        std::future::ready(self.messages.pop_front())
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }
}

#[derive(Clone, Default)]
struct RecordingAgent {
    events: Arc<Mutex<Vec<TaskStatusNotificationParams>>>,
    delivered: Arc<Notify>,
}

impl ClientHandler for RecordingAgent {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities = ClientCapabilities::builder().enable_tasks().build();
        info
    }

    async fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) {
        self.events.lock().await.push(params);
        self.delivered.notify_one();
    }
}

#[derive(Clone)]
struct TrivialServer;

impl ServerHandler for TrivialServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
    }
}

#[tokio::test]
async fn pending_task_ordered_relay_handler_bounds_queue_and_preserves_subscription_routing() {
    let agent = RecordingAgent::default();
    let (client_transport, server_transport) = tokio::io::duplex(16 * 1_024);
    let connecting = tokio::spawn(agent.clone().serve(client_transport));
    let server = TrivialServer.serve(server_transport).await.unwrap();
    let client = connecting.await.unwrap().unwrap();
    let routes = Arc::new(route_state(small_limits()));
    let (tx, _rx) = tokio::sync::broadcast::channel(1);
    let handler = RelayClientHandler::new_with_routes(
        server.peer().clone(),
        Arc::from("pending-fixture"),
        agent.get_info().capabilities,
        Arc::clone(&routes),
        tx,
        false,
    );
    let mut messages = VecDeque::new();
    for message in ["first", "second", "overflow"] {
        messages.push_back(RxJsonRpcMessage::<RoleClient>::notification(
            ServerNotification::TaskStatusNotification(TaskStatusNotification::new(status(
                "native", message,
            ))),
        ));
    }
    let mut subscribed = ServerNotification::TaskStatusNotification(TaskStatusNotification::new(
        status("subscription-only", "subscribed"),
    ));
    subscribed
        .get_meta_mut()
        .set_subscription_id(NumberOrString::Number(77));
    messages.push_back(RxJsonRpcMessage::<RoleClient>::notification(subscribed));
    let mut ordered = OrderedRelayNotificationTransport::new(
        OrderedFixture { messages },
        Some(handler.notification_interceptor()),
    );
    let passed = tokio::time::timeout(Duration::from_secs(2), ordered.receive())
        .await
        .unwrap()
        .unwrap();
    let subscription = match passed {
        JsonRpcMessage::Notification(notification) => {
            notification.notification.get_meta().subscription_id()
        }
        _ => None,
    };
    assert_eq!(subscription, Some(NumberOrString::Number(77)));
    assert!(
        agent.events.lock().await.is_empty(),
        "unregistered events are not exposed downstream"
    );
    assert!(
        routes
            .register_task_id("subscription-only", "gateway-subscription")
            .await
            .is_empty()
    );
    let pending = routes.register_task_id("native", "gateway-native").await;
    assert_eq!(pending.len(), limits(&routes).await.per_id);
    for event in pending {
        handler.forward_task_status(event).await;
    }
    handler
        .handle_task_status(status("native", "mapped-latest"))
        .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let ready = agent.delivered.notified();
            if agent.events.lock().await.len() >= 3 {
                break;
            }
            ready.await;
        }
    })
    .await
    .unwrap();
    let observed = agent.events.lock().await;
    assert_eq!(
        observed
            .iter()
            .map(|event| event.task.task.status_message.as_deref())
            .collect::<Vec<_>>(),
        [Some("first"), Some("second"), Some("mapped-latest")]
    );
    assert!(
        observed
            .iter()
            .all(|event| event.task.task.task_id == "gateway-native")
    );
    drop(observed);
    drop(ordered);
    server.cancel().await.unwrap();
    client.cancel().await.unwrap();
}
