//! Caller-requested Code Mode response budgets for programmatic MCP clients.
//!
//! `code_mode.max_response_bytes` (24 KiB by default) exists to protect an
//! agent's context window. A program that consumes the result itself, such as
//! a dashboard running fixed read-only scripts, has no context window to
//! protect. It can ask for a larger envelope budget on one `tools/call` by
//! sending protocol metadata:
//!
//! ```json
//! { "_meta": { "ai.dinglebear.labby/responseBudget": { "maxResponseBytes": 1048576 } } }
//! ```
//!
//! The request travels in `params._meta`, not in tool `arguments`. A model
//! chooses tool arguments; the MCP client program owns `_meta`. So an agent
//! cannot raise its own budget by asking, and callers that send nothing keep
//! the configured default. The operator bounds every request with
//! `code_mode.max_programmatic_response_bytes`; `0` turns requests off.
//!
//! This is a context-size control, not an authorization boundary. It grants no
//! new data: the same caller could already read the full output through
//! artifacts or paginated reads. It changes only how much of the final
//! envelope is returned inline.

use serde_json::Value;

use crate::gateway_config::CodeModeConfig;

/// `tools/call` `_meta` key that carries a programmatic response budget.
pub const RESPONSE_BUDGET_META_KEY: &str = "ai.dinglebear.labby/responseBudget";

/// Field inside [`RESPONSE_BUDGET_META_KEY`] holding the requested byte budget.
pub const RESPONSE_BUDGET_BYTES_FIELD: &str = "maxResponseBytes";

/// Smallest budget a caller may request. Matches the configured floor.
pub const MIN_REQUESTED_RESPONSE_BYTES: usize = 1024;

/// Default for `code_mode.max_programmatic_response_bytes` (1 MiB).
pub const DEFAULT_MAX_PROGRAMMATIC_RESPONSE_BYTES: usize = 1024 * 1024;

/// Hard ceiling for `code_mode.max_programmatic_response_bytes` (4 MiB).
///
/// A Code Mode MCP result carries the envelope twice (escaped text plus
/// structured trace). Streamable HTTP clients such as the stdio bridge reject
/// JSON bodies over 16 MiB, so 4 MiB keeps the worst case under that limit.
pub const MAX_PROGRAMMATIC_RESPONSE_BYTES_CEILING: usize = 4 * 1024 * 1024;

/// Effective response budget for one Code Mode execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeModeResponseBudget {
    /// Maximum serialized response envelope size in bytes.
    pub max_response_bytes: usize,
    /// Approximate maximum response tokens.
    pub max_response_tokens: usize,
    /// Whether the caller requested this budget through request metadata.
    pub requested: bool,
}

/// A rejected programmatic response-budget request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResponseBudgetError {
    /// The metadata value is not `{ "maxResponseBytes": <positive integer> }`.
    #[error(
        "`_meta[\"ai.dinglebear.labby/responseBudget\"]` must be an object with an integer `maxResponseBytes`; {reason}"
    )]
    Malformed {
        /// What was wrong with the value.
        reason: &'static str,
    },
    /// The operator turned programmatic budgets off.
    #[error(
        "programmatic Code Mode response budgets are disabled on this gateway (code_mode.max_programmatic_response_bytes = 0); remove `_meta[\"ai.dinglebear.labby/responseBudget\"]` to use the default {default_bytes}-byte budget"
    )]
    Disabled {
        /// The default budget the caller would get without the request.
        default_bytes: usize,
    },
    /// The request is smaller than the supported floor.
    #[error(
        "requested Code Mode response budget {requested} bytes is below the minimum {minimum} bytes"
    )]
    BelowMinimum {
        /// Requested bytes.
        requested: u64,
        /// Smallest accepted request.
        minimum: usize,
    },
    /// The request is larger than the operator's hard maximum.
    #[error(
        "requested Code Mode response budget {requested} bytes exceeds this gateway's maximum {maximum} bytes (code_mode.max_programmatic_response_bytes); request {maximum} or less"
    )]
    AboveMaximum {
        /// Requested bytes.
        requested: u64,
        /// Largest accepted request.
        maximum: usize,
    },
}

impl ResponseBudgetError {
    /// The structured parameter path callers should fix.
    pub const PARAM: &'static str = "_meta.ai.dinglebear.labby/responseBudget.maxResponseBytes";
}

/// Parse the requested byte budget from a `_meta` value.
///
/// `None` (key absent) means the caller made no request.
///
/// # Errors
///
/// Returns [`ResponseBudgetError::Malformed`] when the value has the wrong shape.
pub fn requested_response_bytes(value: Option<&Value>) -> Result<Option<u64>, ResponseBudgetError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let object = value.as_object().ok_or(ResponseBudgetError::Malformed {
        reason: "the value is not an object",
    })?;
    let bytes = object
        .get(RESPONSE_BUDGET_BYTES_FIELD)
        .ok_or(ResponseBudgetError::Malformed {
            reason: "`maxResponseBytes` is missing",
        })?;
    bytes
        .as_u64()
        .map(Some)
        .ok_or(ResponseBudgetError::Malformed {
            reason: "`maxResponseBytes` is not a non-negative integer",
        })
}

impl CodeModeConfig {
    /// Effective serialized envelope length under both configured response limits.
    #[must_use]
    pub fn response_envelope_max_bytes(&self) -> usize {
        self.max_response_bytes.min(
            self.max_response_tokens
                .saturating_mul(self.token_estimate_divisor.max(1) as usize),
        )
    }

    /// Resolve the response budget for one execution.
    ///
    /// With no request, the configured `max_response_bytes` and
    /// `max_response_tokens` apply unchanged. A request must lie within
    /// `MIN_REQUESTED_RESPONSE_BYTES..=max_programmatic_response_bytes`. A
    /// granted request lifts the token estimate to the byte budget so the
    /// agent-oriented token cap cannot truncate a program's result.
    ///
    /// # Errors
    ///
    /// Returns [`ResponseBudgetError`] when requests are disabled or the
    /// request is outside the accepted range.
    pub fn resolve_response_budget(
        &self,
        requested: Option<u64>,
    ) -> Result<CodeModeResponseBudget, ResponseBudgetError> {
        let Some(requested) = requested else {
            return Ok(CodeModeResponseBudget {
                max_response_bytes: self.max_response_bytes,
                max_response_tokens: self.max_response_tokens,
                requested: false,
            });
        };
        let maximum = self.max_programmatic_response_bytes;
        if maximum == 0 {
            return Err(ResponseBudgetError::Disabled {
                default_bytes: self.max_response_bytes,
            });
        }
        if requested < MIN_REQUESTED_RESPONSE_BYTES as u64 {
            return Err(ResponseBudgetError::BelowMinimum {
                requested,
                minimum: MIN_REQUESTED_RESPONSE_BYTES,
            });
        }
        let bytes = usize::try_from(requested)
            .ok()
            .filter(|bytes| *bytes <= maximum)
            .ok_or(ResponseBudgetError::AboveMaximum { requested, maximum })?;
        Ok(CodeModeResponseBudget {
            max_response_bytes: bytes,
            // Estimated tokens never exceed bytes (divisor >= 1).
            max_response_tokens: bytes,
            requested: true,
        })
    }

    /// `self` with the response limits replaced by `budget`.
    #[must_use]
    pub fn with_response_budget(&self, budget: CodeModeResponseBudget) -> Self {
        Self {
            max_response_bytes: budget.max_response_bytes,
            max_response_tokens: budget.max_response_tokens,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_byte_ceiling_honors_the_configured_token_divisor() {
        let mut config = CodeModeConfig {
            max_response_bytes: 100,
            max_response_tokens: 10,
            token_estimate_divisor: 2,
            ..Default::default()
        };
        assert_eq!(config.response_envelope_max_bytes(), 20);
        config.token_estimate_divisor = 16;
        assert_eq!(config.response_envelope_max_bytes(), 100);
    }

    #[test]
    fn absent_request_keeps_the_configured_agent_budget() {
        let config = CodeModeConfig::default();
        let budget = config.resolve_response_budget(None).expect("default");
        assert_eq!(budget.max_response_bytes, 24 * 1024);
        assert_eq!(budget.max_response_tokens, config.max_response_tokens);
        assert!(!budget.requested);
        assert_eq!(requested_response_bytes(None), Ok(None));
    }

    #[test]
    fn default_gateway_grants_requests_up_to_one_mebibyte() {
        let config = CodeModeConfig::default();
        assert_eq!(
            config.max_programmatic_response_bytes,
            DEFAULT_MAX_PROGRAMMATIC_RESPONSE_BYTES
        );
        let budget = config
            .resolve_response_budget(Some(1024 * 1024))
            .expect("at the maximum");
        assert_eq!(budget.max_response_bytes, 1024 * 1024);
        assert_eq!(budget.max_response_tokens, 1024 * 1024);
        assert!(budget.requested);

        let smaller = config.resolve_response_budget(Some(4096)).expect("smaller");
        assert_eq!(smaller.max_response_bytes, 4096);
    }

    #[test]
    fn request_above_operator_maximum_is_rejected_with_the_maximum() {
        let config = CodeModeConfig {
            max_programmatic_response_bytes: 256 * 1024,
            ..CodeModeConfig::default()
        };
        let error = config
            .resolve_response_budget(Some(256 * 1024 + 1))
            .expect_err("above maximum");
        assert_eq!(
            error,
            ResponseBudgetError::AboveMaximum {
                requested: 256 * 1024 + 1,
                maximum: 256 * 1024,
            }
        );
        assert!(error.to_string().contains("262144"));
        assert!(matches!(
            config.resolve_response_budget(Some(u64::MAX)),
            Err(ResponseBudgetError::AboveMaximum { .. })
        ));
    }

    #[test]
    fn request_below_floor_is_rejected() {
        let config = CodeModeConfig::default();
        assert_eq!(
            config.resolve_response_budget(Some(1023)),
            Err(ResponseBudgetError::BelowMinimum {
                requested: 1023,
                minimum: 1024,
            })
        );
    }

    #[test]
    fn disabled_gateway_rejects_every_request() {
        let config = CodeModeConfig {
            max_programmatic_response_bytes: 0,
            ..CodeModeConfig::default()
        };
        assert_eq!(
            config.resolve_response_budget(Some(4096)),
            Err(ResponseBudgetError::Disabled {
                default_bytes: 24 * 1024
            })
        );
        config
            .resolve_response_budget(None)
            .expect("no request still works");
    }

    #[test]
    fn malformed_metadata_is_rejected() {
        for value in [
            json!(1_048_576),
            json!({}),
            json!({ "maxResponseBytes": "1048576" }),
            json!({ "maxResponseBytes": -1 }),
            json!({ "maxResponseBytes": 1.5 }),
        ] {
            assert!(
                matches!(
                    requested_response_bytes(Some(&value)),
                    Err(ResponseBudgetError::Malformed { .. })
                ),
                "{value} must be rejected"
            );
        }
        assert_eq!(
            requested_response_bytes(Some(&json!({ "maxResponseBytes": 65536 }))),
            Ok(Some(65536))
        );
    }

    #[test]
    fn with_response_budget_overrides_only_response_limits() {
        let config = CodeModeConfig::default();
        let budget = config
            .resolve_response_budget(Some(500_000))
            .expect("grant");
        let next = config.with_response_budget(budget);
        assert_eq!(next.max_response_bytes, 500_000);
        assert_eq!(next.max_response_tokens, 500_000);
        assert_eq!(next.timeout_ms, config.timeout_ms);
        assert_eq!(
            next.max_programmatic_response_bytes,
            config.max_programmatic_response_bytes
        );
    }

    #[test]
    fn operator_maximum_validation_is_bounded() {
        let mut config = CodeModeConfig {
            max_programmatic_response_bytes: 0,
            ..CodeModeConfig::default()
        };
        config.validate().expect("0 disables requests");
        config.max_programmatic_response_bytes = 1023;
        assert!(matches!(
            config.validate(),
            Err(
                crate::gateway_config::ConfigError::InvalidCodeModeMaxProgrammaticResponseBytes {
                    value: 1023
                }
            )
        ));
        config.max_programmatic_response_bytes = MAX_PROGRAMMATIC_RESPONSE_BYTES_CEILING;
        config.validate().expect("ceiling validates");
        config.max_programmatic_response_bytes = MAX_PROGRAMMATIC_RESPONSE_BYTES_CEILING + 1;
        assert!(matches!(
            config.validate(),
            Err(
                crate::gateway_config::ConfigError::InvalidCodeModeMaxProgrammaticResponseBytes { .. }
            )
        ));
    }

    #[test]
    fn operator_maximum_reads_from_toml_with_default() {
        let config: CodeModeConfig = toml::from_str("").expect("defaults");
        assert_eq!(config.max_programmatic_response_bytes, 1024 * 1024);
        let config: CodeModeConfig =
            toml::from_str("max_programmatic_response_bytes = 0").expect("disabled");
        assert_eq!(config.max_programmatic_response_bytes, 0);
    }
}
