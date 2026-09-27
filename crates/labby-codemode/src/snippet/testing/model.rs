//! Portable fixture contracts. Fixtures contain JSON values, never host file paths.
use super::super::tool_declarations::SnippetToolDeclarations;
use crate::error::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One deterministic, offline snippet test.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnippetTestCase {
    /// Input merged with the snippet's declared defaults.
    #[serde(default = "empty_object")]
    pub input: Value,
    /// Ordered response rules; matching rules are consumed exactly times times.
    #[serde(default)]
    pub calls: Vec<MockCall>,
    /// Exact result assertions keyed by RFC 6901 JSON pointers.
    #[serde(default)]
    pub expect: BTreeMap<String, Value>,
    /// Result pointers which must not exist (different from explicit null).
    #[serde(default)]
    pub absent: Vec<String>,
    /// Optional complete expected result, after normalization on both sides.
    #[serde(default)]
    pub snapshot: Option<Value>,
    /// JSON pointers replaced with null before snapshot comparison only.
    #[serde(default)]
    pub normalize: Vec<String>,
    /// Resource ceilings checked against the unshaped result and actual trace.
    #[serde(default)]
    pub budgets: TestBudgets,
}
fn empty_object() -> Value {
    serde_json::json!({})
}
fn one() -> usize {
    1
}

/// A finite response rule. Omitted params matches any argument object.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockCall {
    /// Exact upstream::tool identifier; reserved providers are rejected.
    pub tool: String,
    /// Exact expected JSON arguments, or omitted for a wildcard rule.
    #[serde(default)]
    pub params: Option<Value>,
    /// JSON returned by each matching call.
    #[serde(default)]
    pub response: Value,
    /// Optional simulated failure instead of a response.
    #[serde(default)]
    pub error: Option<MockError>,
    /// Required consumption count. Rules cannot be unbounded.
    #[serde(default = "one")]
    pub times: usize,
}
/// Controlled synthetic upstream error.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MockError {
    /// Stable simulated error kind.
    pub kind: String,
    /// Synthetic message; do not put credentials in fixtures.
    pub message: String,
}
/// A test's wall time, host-call count and serialized-result budgets.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TestBudgets {
    /// Wall time, including catalog construction and runner startup.
    pub wall_clock_ms: u64,
    /// Maximum attempted host calls.
    pub tool_calls: usize,
    /// Maximum UTF-8 bytes of the result, before display shaping.
    pub output_bytes: usize,
}
impl Default for TestBudgets {
    fn default() -> Self {
        Self {
            wall_clock_ms: 20_000,
            tool_calls: 40,
            output_bytes: 16_000,
        }
    }
}
impl SnippetTestCase {
    /// Reject invalid fixtures before starting a runner or constructing a host.
    pub fn validate(&self) -> Result<(), ToolError> {
        if !self.input.is_object() {
            return Err(invalid("fixture input must be an object"));
        }
        let b = &self.budgets;
        if b.wall_clock_ms == 0
            || b.wall_clock_ms > 30_000
            || b.tool_calls > 512
            || b.output_bytes == 0
            || b.output_bytes > 24_576
        {
            return Err(invalid(
                "budgets require wall_clock_ms 1..30000, tool_calls 0..512, output_bytes 1..24576",
            ));
        }
        if self.calls.len() > 512
            || self.expect.len() + self.absent.len() + self.normalize.len() > 512
        {
            return Err(invalid("fixture has too many rules or assertions"));
        }
        let ids = self
            .calls
            .iter()
            .map(|c| c.tool.clone())
            .collect::<std::collections::BTreeSet<_>>();
        SnippetToolDeclarations::try_from(ids.into_iter().collect::<Vec<_>>())?;
        for c in &self.calls {
            if c.times == 0 || c.times > 512 || c.params.as_ref().is_some_and(|v| !v.is_object()) {
                return Err(invalid(
                    "mock times must be 1..512 and params must be an object",
                ));
            }
        }
        for p in self
            .expect
            .keys()
            .chain(self.absent.iter())
            .chain(self.normalize.iter())
        {
            if !valid_pointer(p) {
                return Err(invalid("assertions must use valid RFC 6901 JSON pointers"));
            }
        }
        Ok(())
    }
}
fn valid_pointer(p: &str) -> bool {
    if !p.is_empty() && !p.starts_with('/') {
        return false;
    }
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}
pub(super) fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.to_string(),
        param: "fixture".to_string(),
    }
}
