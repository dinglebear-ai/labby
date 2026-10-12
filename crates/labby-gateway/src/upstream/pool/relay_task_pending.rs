//! Bounded retention for task status received before public ID registration.
//!
//! Notifications are advisory: overflow and expiry fall back to existing task
//! polling, never replay of task creation. Expiry is checked on receipt/drain;
//! a silent peer keeps at most the capped buffer until its next operation/drop.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::time::Duration;

use rmcp::model::TaskStatusNotificationParams;
use serde::Serialize;
use tokio::time::Instant;

const DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(30);

#[path = "relay_task_pending_wire.rs"]
mod wire;

#[derive(Clone, Copy)]
pub(super) struct PendingTaskLimits {
    pub(super) ids: usize,
    pub(super) per_id: usize,
    pub(super) count: usize,
    pub(super) bytes: usize,
    pub(super) ttl: Duration,
}

impl Default for PendingTaskLimits {
    fn default() -> Self {
        Self {
            ids: 64,
            per_id: 4,
            count: 128,
            bytes: 64 * 1_024,
            ttl: Duration::from_secs(30),
        }
    }
}

struct PendingEvent {
    params: TaskStatusNotificationParams,
    received: Instant,
    bytes: usize,
}

struct PendingId {
    events: VecDeque<PendingEvent>,
    key_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DiscardCounts {
    capacity: u64,
    bytes: u64,
    expired: u64,
}

#[derive(Default)]
pub(super) struct PendingTaskBuffer {
    ids: HashMap<String, PendingId>,
    count: usize,
    // Upper bound for serialized {id:[params,...],...}, excluding root braces.
    // Counts bound container/object overhead independently of serialized bytes.
    bytes: usize,
    discarded: DiscardCounts,
    last_diagnostic: Option<Instant>,
}

impl PendingTaskBuffer {
    pub(super) fn admit(
        &mut self,
        params: TaskStatusNotificationParams,
        limits: PendingTaskLimits,
        now: Instant,
    ) {
        self.expire(now, limits.ttl);
        let id = params.task.task.task_id.as_str();
        let existing = self.ids.get(id);
        if self.count >= limits.count
            || existing.map_or(0, |pending| pending.events.len()) >= limits.per_id
            || (existing.is_none() && self.ids.len() >= limits.ids)
        {
            self.discarded.capacity = self.discarded.capacity.saturating_add(1);
            self.report_discards(now);
            return;
        }

        // Reserve root braces and conservative commas before counting. The
        // capped writer uses a borrowed wire view, bypassing RMCP's cloning task
        // and request serializers. An unknown ID clones only after admission.
        let remaining = limits.bytes.saturating_sub(self.bytes.saturating_add(2));
        let key_bytes = if existing.is_none() {
            match remaining.checked_sub(4).and_then(|cap| json_size(id, cap)) {
                Some(bytes) => bytes + 4, // colon, array brackets, group comma
                None => {
                    self.discard_bytes(now);
                    return;
                }
            }
        } else {
            0
        };
        let event_bytes = match remaining
            .checked_sub(key_bytes)
            .and_then(|cap| cap.checked_sub(1))
            .and_then(|cap| notification_json_size(&params, cap))
        {
            Some(bytes) => bytes + 1, // conservative event comma
            None => {
                self.discard_bytes(now);
                return;
            }
        };
        let pending = self.ids.entry(id.to_owned()).or_insert_with(|| PendingId {
            events: VecDeque::new(),
            key_bytes,
        });
        pending.events.push_back(PendingEvent {
            params,
            received: now,
            bytes: event_bytes,
        });
        self.count += 1;
        self.bytes += key_bytes + event_bytes;
        self.report_discards(now);
    }

    pub(super) fn drain(
        &mut self,
        id: &str,
        now: Instant,
        ttl: Duration,
    ) -> Vec<TaskStatusNotificationParams> {
        self.expire(now, ttl);
        let Some(pending) = self.ids.remove(id) else {
            return Vec::new();
        };
        self.count -= pending.events.len();
        self.bytes -= pending.key_bytes
            + pending
                .events
                .iter()
                .map(|event| event.bytes)
                .sum::<usize>();
        pending
            .events
            .into_iter()
            .map(|event| event.params)
            .collect()
    }

    pub(super) fn expire(&mut self, now: Instant, ttl: Duration) {
        let mut expired_count = 0;
        let mut expired_bytes = 0;
        // At most 64 IDs/128 events in production, and no awaited work.
        self.ids.retain(|_, pending| {
            while pending
                .events
                .front()
                .is_some_and(|event| now.saturating_duration_since(event.received) >= ttl)
            {
                if let Some(event) = pending.events.pop_front() {
                    expired_count += 1;
                    expired_bytes += event.bytes;
                }
            }
            if pending.events.is_empty() {
                expired_bytes += pending.key_bytes;
                false
            } else {
                true
            }
        });
        self.count -= expired_count;
        self.bytes -= expired_bytes;
        self.discarded.expired = self.discarded.expired.saturating_add(expired_count as u64);
        self.report_discards(now);
    }

    fn discard_bytes(&mut self, now: Instant) {
        self.discarded.bytes = self.discarded.bytes.saturating_add(1);
        self.report_discards(now);
    }

    fn take_diagnostic(&mut self, now: Instant) -> Option<DiscardCounts> {
        if self.discarded == DiscardCounts::default()
            || self
                .last_diagnostic
                .is_some_and(|last| now.saturating_duration_since(last) < DIAGNOSTIC_INTERVAL)
        {
            return None;
        }
        self.last_diagnostic = Some(now);
        Some(std::mem::take(&mut self.discarded))
    }

    fn report_discards(&mut self, now: Instant) {
        if let Some(counts) = self.take_diagnostic(now) {
            tracing::warn!(
                capacity_drops = counts.capacity,
                byte_drops = counts.bytes,
                expired_events = counts.expired,
                "unmapped task status discarded; recover through task polling"
            );
        }
    }
}

struct CappedByteCounter {
    used: usize,
    cap: usize,
}

impl Write for CappedByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.cap - self.used {
            return Err(io::Error::other("pending task notification byte limit"));
        }
        self.used += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn json_size(value: &(impl Serialize + ?Sized), cap: usize) -> Option<usize> {
    let mut counter = CappedByteCounter { used: 0, cap };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.used)
}

fn notification_json_size(params: &TaskStatusNotificationParams, cap: usize) -> Option<usize> {
    json_size(&wire::TaskParamsWire::new(params)?, cap)
}

#[cfg(test)]
#[path = "relay_task_pending_unit_tests.rs"]
mod tests;
