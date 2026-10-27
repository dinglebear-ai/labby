//! Byte accounting, expiry and fixed diagnostic state for the pending helper.

use std::collections::BTreeMap;

use rmcp::model::{DetailedTask, Task, TaskPayload, TaskStatus};

use super::*;

fn limits() -> PendingTaskLimits {
    PendingTaskLimits {
        ids: 2,
        per_id: 3,
        count: 4,
        bytes: 2_048,
        ttl: Duration::from_secs(1),
    }
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

#[test]
fn pending_task_counter_stops_without_retaining_or_counting_rejected_chunks() {
    let mut counter = CappedByteCounter { used: 0, cap: 4 };
    assert_eq!(counter.write(b"one").unwrap(), 3);
    assert!(counter.write(b"overflow").is_err());
    assert_eq!(counter.used, 3);
    assert_eq!(counter.write(b"x").unwrap(), 1);
    assert!(counter.write(b"y").is_err());
    assert_eq!(counter.used, counter.cap);
}

#[test]
fn pending_task_counter_matches_json_utf8_and_escaping_at_exact_budget() {
    let event = status("id-\"-é", "é\n\"\t\\".repeat(8).as_str());
    let serialized = serde_json::to_vec(&event).unwrap();
    assert_eq!(
        notification_json_size(&event, serialized.len()),
        Some(serialized.len())
    );
    assert!(notification_json_size(&event, serialized.len() - 1).is_none());
    assert!(json_size(&"x".repeat(4_096), 16).is_none());
}

#[test]
fn pending_task_retained_json_is_below_accounted_budget_and_drain_releases_it() {
    let now = Instant::now();
    let mut buffer = PendingTaskBuffer::default();
    for (id, message) in [("a\"", "é\n"), ("a\"", "later"), ("b", "other")] {
        buffer.admit(status(id, message), limits(), now);
    }
    let count = buffer.count;
    let accounted = buffer.bytes + 2;
    assert_eq!(count, 3);
    assert!(accounted <= limits().bytes);
    let retained = BTreeMap::from([
        ("a\"", buffer.drain("a\"", now, limits().ttl)),
        ("b", buffer.drain("b", now, limits().ttl)),
    ]);
    assert!(serde_json::to_vec(&retained).unwrap().len() <= accounted);
    assert!(buffer.ids.is_empty());
    assert_eq!(buffer.count, 0);
    assert_eq!(buffer.bytes, 0);
    buffer.admit(status("fresh", "early"), limits(), now);
    assert_eq!(buffer.drain("fresh", now, limits().ttl).len(), 1);
}

#[test]
fn pending_task_expiry_removes_old_prefix_and_empty_id_budget() {
    let now = Instant::now();
    let mut buffer = PendingTaskBuffer {
        last_diagnostic: Some(now),
        ..Default::default()
    };
    buffer.admit(status("a", "old"), limits(), now);
    buffer.admit(status("b", "old"), limits(), now);
    buffer.admit(
        status("a", "newer"),
        limits(),
        now + Duration::from_millis(500),
    );
    let later = now + Duration::from_millis(1_100);
    buffer.expire(later, limits().ttl);
    assert_eq!(buffer.ids.len(), 1);
    assert_eq!(buffer.count, 1);
    assert_eq!(buffer.discarded.expired, 2);
    assert!(buffer.drain("b", later, limits().ttl).is_empty());
    let retained = buffer.drain("a", later, limits().ttl);
    assert_eq!(retained.len(), 1);
    assert_eq!(
        retained[0].task.task.status_message.as_deref(),
        Some("newer")
    );
    assert_eq!(buffer.bytes, 0);
}

#[test]
fn pending_task_registration_checks_ttl_without_new_notification() {
    let now = Instant::now();
    let mut buffer = PendingTaskBuffer::default();
    buffer.admit(status("a", "old"), limits(), now);
    assert!(
        buffer
            .drain("a", now + limits().ttl, limits().ttl)
            .is_empty()
    );
    assert_eq!(buffer.count, 0);
    assert_eq!(buffer.bytes, 0);
    assert!(buffer.ids.is_empty());
}

#[test]
fn pending_task_diagnostic_counts_saturate_and_emit_at_most_once_per_interval() {
    let now = Instant::now();
    let mut buffer = PendingTaskBuffer {
        last_diagnostic: Some(now),
        discarded: DiscardCounts {
            capacity: u64::MAX,
            ..Default::default()
        },
        ..Default::default()
    };
    buffer.admit(
        status("never-retained", "ignored"),
        PendingTaskLimits {
            count: 0,
            ..limits()
        },
        now,
    );
    buffer.discard_bytes(now);
    assert_eq!(buffer.discarded.capacity, u64::MAX);
    assert!(buffer.take_diagnostic(now).is_none());
    let later = now + DIAGNOSTIC_INTERVAL;
    assert_eq!(
        buffer.take_diagnostic(later),
        Some(DiscardCounts {
            capacity: u64::MAX,
            bytes: 1,
            expired: 0,
        })
    );
    buffer.discard_bytes(later);
    assert!(buffer.take_diagnostic(later).is_none());
    assert_eq!(buffer.discarded.bytes, 1);
    assert!(buffer.ids.is_empty());
    assert_eq!(buffer.count, 0);
    assert_eq!(buffer.bytes, 0);
}
