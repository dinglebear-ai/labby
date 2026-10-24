//! HTTP worker budgets shared by TCP, Unix-socket and relay connections.

use std::time::Duration;

use super::helpers::{DEFAULT_REQUEST_TIMEOUT, DISCOVERY_TIMEOUT, upstream_call_concurrency};
use super::relay_cancellation::CANCELLATION_DELIVERY_TIMEOUT;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

#[derive(Clone, Copy)]
pub(super) struct HttpTransportBudgets {
    pub concurrency: usize,
    pub request_timeout: Duration,
}

impl HttpTransportBudgets {
    pub(super) fn config(
        self,
        uri: &str,
        max_response_bytes: usize,
    ) -> StreamableHttpClientTransportConfig {
        let mut config = StreamableHttpClientTransportConfig::with_uri(uri)
            .max_concurrent_requests(self.concurrency)
            .control_request_timeout(self.request_timeout.min(CANCELLATION_DELIVERY_TIMEOUT))
            .session_recovery_timeout(self.request_timeout.min(DISCOVERY_TIMEOUT));
        // The underlying HTTP/Unix client and SSE decoder share one ceiling.
        config.max_sse_event_size = max_response_bytes;
        config
    }
}
impl Default for HttpTransportBudgets {
    fn default() -> Self {
        Self {
            concurrency: upstream_call_concurrency(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::helpers::DEFAULT_MAX_SKILL_RESPONSE_BYTES;
    use super::*;

    #[test]
    fn http_worker_budgets_match_the_configured_upstream_admission_limit() {
        let config = HttpTransportBudgets {
            concurrency: 3,
            request_timeout: Duration::from_secs(30),
        }
        .config("http://localhost/mcp", DEFAULT_MAX_SKILL_RESPONSE_BYTES);
        assert_eq!(config.max_concurrent_requests, 3);
        assert_eq!(
            config.control_request_timeout,
            CANCELLATION_DELIVERY_TIMEOUT
        );
        assert_eq!(config.session_recovery_timeout, DISCOVERY_TIMEOUT);
        assert_eq!(config.max_sse_event_size, DEFAULT_MAX_SKILL_RESPONSE_BYTES);
        assert!(
            config.reinit_on_expired_session,
            "preserve SDK's session-expired-only recovery"
        );
    }

    #[test]
    fn short_request_deadlines_bound_control_and_recovery_work() {
        let timeout = Duration::from_millis(75);
        let config = HttpTransportBudgets {
            concurrency: 1,
            request_timeout: timeout,
        }
        .config("/mcp?tenant=local", 1024);
        assert_eq!(config.uri.as_ref(), "/mcp?tenant=local");
        assert_eq!(config.max_concurrent_requests, 1);
        assert_eq!(config.control_request_timeout, timeout);
        assert_eq!(config.session_recovery_timeout, timeout);
        assert_eq!(config.max_sse_event_size, 1024);
    }
}
