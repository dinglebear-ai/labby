//! Bounded capability generations. An invocation lease protects its peer until
//! response validation and publication finish; revocation still retires all peers.

use std::sync::Arc;

use super::super::relay_cache::RelayCachedConnection;
use super::TaskRoute;

pub(super) const MAX_TASK_GENERATIONS: usize = 4;

pub(in crate::upstream::pool) struct TaskCompanion {
    pub(in crate::upstream::pool) connection: RelayCachedConnection,
    pub(in crate::upstream::pool) invocations: Arc<()>,
}

impl TaskCompanion {
    pub(super) fn new(connection: RelayCachedConnection) -> Self {
        Self {
            connection,
            invocations: Arc::new(()),
        }
    }
}

impl TaskRoute {
    /// Called under the task map writer. Capacity pressure may retire an idle
    /// generation, never a peer leased by an already dispatched invocation.
    pub(super) fn install_companion(
        &mut self,
        connection: RelayCachedConnection,
    ) -> Result<(Arc<()>, Option<RelayCachedConnection>), String> {
        let replace = self.companions.iter().position(|companion| {
            companion.connection.capability_fingerprint == connection.capability_fingerprint
        });
        let replace = if let Some(index) = replace {
            if Arc::strong_count(&self.companions[index].invocations) > 1 {
                return Err("task companion generation busy".to_string());
            }
            Some(index)
        } else if self.companions.len() >= MAX_TASK_GENERATIONS {
            Some(
                self.companions
                    .iter()
                    .enumerate()
                    .filter(|(_, companion)| Arc::strong_count(&companion.invocations) == 1)
                    .min_by_key(|(_, companion)| companion.connection.last_used)
                    .map(|(index, _)| index)
                    .ok_or_else(|| "task companion capacity exhausted".to_string())?,
            )
        } else {
            None
        };
        let companion = TaskCompanion::new(connection);
        let invocation = Arc::clone(&companion.invocations);
        let retired = match replace {
            Some(index) => {
                Some(std::mem::replace(&mut self.companions[index], companion).connection)
            }
            None => {
                self.companions.push(companion);
                None
            }
        };
        Ok((invocation, retired))
    }
}
