//! Bounded buffering for non-MCP HTTP response bodies.

use std::sync::{Arc, OnceLock};

use futures::StreamExt as _;

const RESPONSE_BUDGET_QUANTUM: usize = 1024;
const AGGREGATE_RESPONSE_BUDGET_BYTES: usize = 80 * 1024 * 1024;
const AGGREGATE_RESPONSE_BUDGET_PERMITS: usize =
    AGGREGATE_RESPONSE_BUDGET_BYTES / RESPONSE_BUDGET_QUANTUM;
static GLOBAL_RESPONSE_BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn effective_response_limit(max_bytes: usize) -> usize {
    max_bytes.min(AGGREGATE_RESPONSE_BUDGET_BYTES)
}

/// Failure while buffering a response within its configured byte budget.
#[derive(Debug, thiserror::Error)]
pub enum CappedResponseBodyError {
    #[error("response_too_large: streamed {observed_bytes} bytes, max {max_bytes}")]
    TooLarge {
        observed_bytes: u64,
        max_bytes: usize,
    },
    #[error("upstream response read failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("response_budget_closed")]
    BudgetClosed,
    #[error("response decode failed: {0}")]
    Decode(String),
}

/// Buffer a decoded HTTP response while enforcing per-response and aggregate bounds.
pub async fn read_response_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CappedResponseBodyError> {
    let max_bytes = effective_response_limit(max_bytes);
    let weight = max_bytes
        .div_ceil(RESPONSE_BUDGET_QUANTUM)
        .max(1)
        .min(AGGREGATE_RESPONSE_BUDGET_PERMITS);
    let budget = Arc::clone(GLOBAL_RESPONSE_BUDGET.get_or_init(|| {
        Arc::new(tokio::sync::Semaphore::new(
            AGGREGATE_RESPONSE_BUDGET_PERMITS,
        ))
    }));
    let _permit = budget
        .acquire_many_owned(u32::try_from(weight).unwrap_or(u32::MAX))
        .await
        .map_err(|_| CappedResponseBodyError::BudgetClosed)?;
    if let Some(declared) = response.content_length()
        && declared > max_bytes as u64
    {
        return Err(CappedResponseBodyError::TooLarge {
            observed_bytes: declared,
            max_bytes,
        });
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(max_bytes as u64) as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CappedResponseBodyError::TooLarge {
                observed_bytes: bytes.len().saturating_add(chunk.len()) as u64,
                max_bytes,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_limit_cannot_exceed_the_aggregate_budget() {
        assert_eq!(
            effective_response_limit(AGGREGATE_RESPONSE_BUDGET_BYTES + 1),
            AGGREGATE_RESPONSE_BUDGET_BYTES
        );
        assert_eq!(effective_response_limit(1024), 1024);
    }
}
