#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_tools_transport_qualification.rs"]
mod qualification;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use qualification::{TransportKind, TransportQualification, assert_typed_error, run_failure_safe};
use rmcp::model::{ElicitationAction, ErrorCode};

const SECRET_CANARY: &str = "q1-secret-must-never-cross-the-mcp-boundary";

#[tokio::test]
async fn q1_real_process_mcp_tools_and_transport_lifecycle_matrix() {
    for transport in [TransportKind::Stdio, TransportKind::StreamableHttp] {
        run_failure_safe(transport, SECRET_CANARY, qualify_transport)
            .await
            .unwrap_or_else(|error| panic!("{transport:?} qualification failed: {error}"));
    }
}

fn qualify_transport(
    harness: &mut TransportQualification,
) -> futures::future::BoxFuture<'_, Result<(), String>> {
    Box::pin(async move {
        let transport = harness.kind();

        harness
            .assert_exact_server_identity("labby", env!("CARGO_PKG_VERSION"), "2026-07-28")
            .unwrap_or_else(|error| panic!("{transport:?} identity: {error}"));

        let completion_error = harness
            .call_unsupported_completion()
            .await
            .unwrap_or_else(|error| panic!("{transport:?} completion rejection: {error}"));
        assert_eq!(
            completion_error.code,
            ErrorCode::METHOD_NOT_FOUND,
            "{transport:?}: completion/complete must reject with JSON-RPC -32601"
        );

        let gateway_call = harness
            .call_raw(
                "forge.safe",
                serde_json::json!({"query": "qualification", "limit": 1, "enabled": true}),
            )
            .await
            .unwrap_or_else(|error| panic!("{transport:?} gateway tool wire error: {error}"));
        assert_ne!(
            gateway_call.is_error,
            Some(true),
            "{transport:?}: {gateway_call:?}"
        );
        assert_eq!(
            harness
                .effect_counts()
                .unwrap_or_else(|error| panic!("{transport:?} first effect: {error}")),
            (1, 0),
            "{transport:?}: one successful gateway call must have one settled effect"
        );

        let tools = harness
            .discover_tools()
            .await
            .unwrap_or_else(|error| panic!("{transport:?} tools/list: {error}"));
        assert!(tools.contains_key("doctor"), "{transport:?}: {tools:?}");
        assert!(
            tools.contains_key("forge.delay"),
            "{transport:?}: upstream tool missing from {tools:?}"
        );
        qualification::assert_service_tool_schema(&tools["doctor"])
            .unwrap_or_else(|error| panic!("{transport:?} doctor schema: {error}"));

        let success = harness
            .call_service("doctor", "help", serde_json::json!({}))
            .await
            .unwrap_or_else(|error| panic!("{transport:?} doctor.help wire error: {error}"));
        assert_ne!(success.is_error, Some(true), "{transport:?}: {success:?}");
        assert!(
            qualification::result_text(&success).contains("doctor"),
            "{transport:?}: doctor.help returned no literal service evidence"
        );

        let invalid = harness
            .call_service(
                "doctor",
                "schema",
                serde_json::json!({
                    "action": 17,
                    "credential": SECRET_CANARY
                }),
            )
            .await
            .unwrap_or_else(|error| panic!("{transport:?} invalid-schema wire error: {error}"));
        assert_typed_error(&invalid, "missing_param", SECRET_CANARY)
            .unwrap_or_else(|error| panic!("{transport:?} invalid schema: {error}"));

        let unknown = harness
            .call_service(
                "doctor",
                "doctor.this_action_does_not_exist",
                serde_json::json!({"credential": SECRET_CANARY}),
            )
            .await
            .unwrap_or_else(|error| panic!("{transport:?} unknown-action wire error: {error}"));
        assert_typed_error(&unknown, "unknown_action", SECRET_CANARY)
            .unwrap_or_else(|error| panic!("{transport:?} unknown action: {error}"));

        let denied = harness
            .decline_destructive_service(
                "snippets",
                "snippets.remove",
                serde_json::json!({
                    "name": "must-not-be-removed",
                    "credential": SECRET_CANARY
                }),
            )
            .await
            .unwrap_or_else(|error| panic!("{transport:?} policy-denial wire error: {error}"));
        assert_typed_error(&denied, "confirmation_required", SECRET_CANARY)
            .unwrap_or_else(|error| panic!("{transport:?} policy denial: {error}"));

        harness
            .cancel_delayed_upstream_after_effect()
            .await
            .unwrap_or_else(|error| panic!("{transport:?} cancellation: {error}"));
        assert_eq!(
            harness
                .effect_counts()
                .unwrap_or_else(|error| panic!("{transport:?} effect ledger: {error}")),
            (2, 0),
            "{transport:?}: cancellation must settle the admitted effect exactly once"
        );

        Ok(())
    })
}

#[tokio::test]
async fn q1_stdio_failure_path_cleans_before_reporting_the_failure() {
    let error = run_failure_safe(TransportKind::Stdio, SECRET_CANARY, |harness| {
        Box::pin(async move {
            let result = harness
                .call_raw(
                    "forge.safe",
                    serde_json::json!({"query": "failure-cleanup", "limit": 1}),
                )
                .await?;
            if result.is_error == Some(true) {
                return Err(format!("upstream setup call failed: {result:?}"));
            }
            panic!("intentional failure after upstream descendant spawn");
        })
    })
    .await
    .unwrap_err();

    assert!(
        error.contains("intentional failure after upstream descendant spawn"),
        "failure-safe wrapper lost the primary failure: {error}"
    );
}

#[tokio::test]
async fn q1_elicitation_accept_decline_cancel_malformed_abandon_and_unsupported_matrix() {
    for transport in [TransportKind::Stdio, TransportKind::StreamableHttp] {
        run_failure_safe(transport, SECRET_CANARY, qualify_elicitation)
            .await
            .unwrap_or_else(|error| panic!("{transport:?} elicitation failed: {error}"));

        let unsupported =
            TransportQualification::start_without_elicitation(transport, SECRET_CANARY)
                .await
                .unwrap_or_else(|error| panic!("{transport:?} unsupported client setup: {error}"));
        unsupported
            .call_raw(
                "forge.safe",
                serde_json::json!({"query":"unsupported-baseline","limit":1}),
            )
            .await
            .expect("unsupported-client baseline call");
        let before = unsupported
            .effect_counts()
            .expect("unsupported before effects");
        let refusal = unsupported
            .call_destructive_without_elicitation()
            .await
            .expect("unsupported-client missing-capability response");
        let after = unsupported
            .effect_counts()
            .expect("unsupported after effects");
        let cleanup = unsupported.finish().await;
        assert_eq!(
            refusal.code,
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
            "2026-07-28 clients missing form elicitation must receive the protocol capability error"
        );
        assert_eq!(
            refusal
                .data
                .as_ref()
                .and_then(|data| data.get("requiredCapabilities"))
                .and_then(|caps| caps.get("elicitation"))
                .and_then(|elicitation| elicitation.get("form")),
            Some(&serde_json::json!({})),
            "missing-capability errors must identify elicitation.form"
        );
        assert_eq!(after, before, "unsupported client must cause no effect");
        assert!(
            cleanup.is_clean(),
            "{transport:?} cleanup: {:?}",
            cleanup.failures
        );
    }
}

fn qualify_elicitation(
    harness: &mut TransportQualification,
) -> futures::future::BoxFuture<'_, Result<(), String>> {
    Box::pin(async move {
        let baseline = harness
            .call_raw(
                "forge.safe",
                serde_json::json!({"query":"elicitation-baseline","limit":1}),
            )
            .await?;
        if baseline.is_error == Some(true) {
            return Err(format!("elicitation baseline failed: {baseline:?}"));
        }
        let initial = harness.effect_counts()?;
        harness.abandon_destructive_challenge().await?;
        if harness.effect_counts()? != initial {
            return Err("abandoned elicitation challenge caused an effect".into());
        }

        for (label, action) in [
            ("declined", ElicitationAction::Decline),
            ("cancelled", ElicitationAction::Cancel),
        ] {
            let refusal = harness.answer_destructive_upstream(action, None).await?;
            assert_typed_error(&refusal, "confirmation_required", SECRET_CANARY)?;
            if harness.effect_counts()? != initial {
                return Err(format!("{label} elicitation caused an effect"));
            }
        }

        let malformed = harness.answer_destructive_upstream_malformed().await?;
        assert_typed_error(&malformed, "confirmation_required", SECRET_CANARY)?;
        if harness.effect_counts()? != initial {
            return Err("malformed elicitation response caused an effect".into());
        }

        let accepted = harness
            .answer_destructive_upstream(
                ElicitationAction::Accept,
                Some(serde_json::json!({"confirm": true})),
            )
            .await?;
        if accepted.is_error == Some(true) {
            return Err(format!("accepted elicitation failed: {accepted:?}"));
        }
        let expected = (initial.0 + 1, 0);
        if harness.effect_counts()? != expected {
            return Err(format!(
                "accepted elicitation did not settle exactly once: {:?}",
                harness.effect_counts()?
            ));
        }
        Ok(())
    })
}
