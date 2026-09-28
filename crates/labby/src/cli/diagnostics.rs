//! Human diagnostics rendered from the same recovery contract as JSON errors.
//! This layer does not classify failures or invent retry/side-effect guarantees.

use labby_runtime::agent_error::sanitize_error_text;
use serde_json::Value;

fn text(value: &Value, fallback: &str) -> String {
    sanitize_error_text(value.as_str().unwrap_or(fallback), 4096)
}

/// Render one failure, including its recovery advice even when tracing is disabled.
#[must_use]
pub fn render_failure(value: &Value) -> String {
    let error = &value["error"];
    let kind = text(&error["kind"], "unknown");
    let message = text(&error["message"], "The command failed.");
    let command = text(&value["command"], "cli");
    let origin = text(&error["origin"], "unknown");
    let effects = match error["side_effects"].as_str() {
        Some("none_expected") => "No operation side effects are expected.",
        Some("possible") => {
            "Partial side effects are possible. Check current state before retrying."
        }
        _ => "Side effects are unknown. Check current state before retrying.",
    };
    let guidance = text(
        &error["recovery"]["guidance"],
        "Inspect diagnostics before retrying.",
    );
    let mut output = format!(
        "Error [{kind}]: {message}\nCommand: labby {command}\nOrigin: {origin}\nEffects: {effects}\nNext: {guidance}"
    );
    if let Some(cause) = error["cause"].as_str() {
        let cause = sanitize_error_text(cause, 4096);
        if !cause.is_empty() && !message.contains(&cause) {
            output.push_str("\nDetails: ");
            output.push_str(&cause);
        }
    }
    if let Some(request_id) = value["request_id"].as_str() {
        output.push_str("\nRequest: ");
        output.push_str(&sanitize_error_text(request_id, 128));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn human_error_preserves_recovery_and_does_not_claim_unknown_effects_are_safe() {
        let value = json!({"command":"server restart", "request_id":"test-request", "error": {
            "kind":"timeout", "message":"The selected gateway did not respond.",
            "origin":"upstream_transport", "side_effects":"unknown",
            "recovery":{"guidance":"Check gateway state before retrying."}
        }});
        let output = render_failure(&value);
        assert!(output.contains("server restart"));
        assert!(output.contains("timeout"));
        assert!(output.contains("Side effects are unknown"));
        assert!(output.contains("Check gateway state"));
        assert!(output.contains("test-request"));
        assert!(!output.contains("No operation side effects"));
    }

    #[test]
    fn human_error_redacts_secret_patterns_and_control_characters() {
        let value = json!({"command":"code run", "error": {
            "kind":"provider_error", "message":"Failed with Bearer abcdefghijklmnop\u{001b}[31m",
            "side_effects":"possible", "recovery":{"guidance":"Inspect the gateway."}
        }});
        let output = render_failure(&value);
        assert!(!output.contains("abcdefghijklmnop"));
        assert!(!output.contains('\u{001b}'));
        assert!(output.contains("Partial side effects"));
    }
}
