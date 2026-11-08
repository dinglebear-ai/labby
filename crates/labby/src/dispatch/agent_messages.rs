//! Session-scoped messages share the durable notice transaction and delivery paths.
mod store;
mod types;

use rusqlite::TransactionBehavior;
use serde_json::Value;

use super::codemode_notices::{NoticeError, NoticeStore, notice_write_error, now_ms};
pub(crate) use types::{MessageEnrollment, MessageRequest, SessionBinding, SessionKind};

impl NoticeStore {
    pub(crate) async fn message_request(
        &self,
        binding: SessionBinding,
        request: MessageRequest,
    ) -> Result<Value, NoticeError> {
        binding.validate()?;
        request.validate()?;
        let publisher = self.clone();
        self.run(move |db| {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| notice_write_error(error, true))?;
            let result = store::execute_in(&tx, &binding, request, now_ms())?;
            tx.commit()
                .map_err(|error| notice_write_error(error, true))?;
            // The blocking transaction can outlast the caller's response
            // budget. Wake delivery based on commit, not on that response.
            if result.wake {
                publisher.notify_published();
            }
            Ok(result.value)
        })
        .await
    }

    pub(crate) async fn renew_message_session(
        &self,
        binding: SessionBinding,
        close: bool,
    ) -> Result<(), NoticeError> {
        binding.validate()?;
        if close {
            binding.close();
        }
        self.run_event(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            store::renew_in(&tx, &binding, close, now_ms())?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn registered_message_recipient(
        &self,
        binding: SessionBinding,
    ) -> Result<MessageEnrollment, NoticeError> {
        binding.validate()?;
        if binding.kind != SessionKind::Http {
            return Err(NoticeError::Identity);
        }
        self.run(move |db| store::enrollment(db, binding, now_ms()))
            .await
    }
}

#[cfg(test)]
mod tests;
