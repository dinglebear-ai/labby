use super::*;
use serde_json::json;

fn request(value: serde_json::Value) -> CodeModeRunnerOutput {
    serde_json::from_value(value).expect("request fixture")
}

fn reply(value: serde_json::Value) -> CodeModeRunnerInput {
    serde_json::from_value(value).expect("reply fixture")
}

#[test]
fn every_native_request_kind_requires_its_consumed_reply() {
    for (out, input) in [
        (
            json!({"type":"tool_call","seq":0,"id":"t","params":{}}),
            json!({"type":"tool_result","seq":0,"result":null}),
        ),
        (
            json!({"type":"artifact_write","seq":1,"path":"owned","content":"v"}),
            json!({"type":"tool_result","seq":1,"result":{}}),
        ),
        (
            json!({"type":"snippet_resolve","seq":2,"name":"s"}),
            json!({"type":"snippet_resolved","seq":2,"code":"async () => 1","input":{}}),
        ),
        (
            json!({"type":"step_begin","seq":3,"name":"s"}),
            json!({"type":"step_decision","seq":3,"replay":null}),
        ),
        (
            json!({"type":"step_result","seq":3,"value":1}),
            json!({"type":"step_recorded","seq":3}),
        ),
    ] {
        let mut ownership = ReplyOwnership::default();
        ownership.record_request(&request(out.clone()));
        assert!(
            !ownership.is_clean(),
            "an emitted request needs acknowledgment"
        );
        ownership.record_reply(&reply(input));
        assert!(ownership.is_clean(), "matching native reply is consumed");

        let mut rejected = ReplyOwnership::default();
        rejected.record_request(&request(out.clone()));
        rejected.record_reply(&reply(json!({"type":"tool_error", "seq":out["seq"],
            "kind":"tool_error","message":"caught"})));
        assert!(rejected.is_clean(), "caught rejection can be reusable");
    }
}

#[test]
fn step_phases_reuse_seq_and_duplicate_emissions_require_two_replies() {
    let mut ownership = ReplyOwnership::default();
    ownership.record_request(&request(json!({"type":"step_begin","seq":7,"name":"s"})));
    ownership.record_reply(&reply(json!({"type":"step_decision","seq":7})));
    assert!(ownership.is_clean());
    let result = request(json!({"type":"step_result","seq":7,"value":4}));
    ownership.record_request(&result);
    ownership.record_request(&result);
    ownership.record_reply(&reply(json!({"type":"step_recorded","seq":7})));
    assert!(
        !ownership.is_clean(),
        "one ack cannot settle two emitted frames"
    );
    ownership.record_reply(&reply(json!({"type":"step_recorded","seq":7})));
    assert!(ownership.is_clean());
}

#[test]
fn duplicate_unexpected_and_wrong_phase_replies_permanently_disqualify_reuse() {
    for unexpected in [
        json!({"type":"tool_result","seq":9,"result":null}),
        json!({"type":"step_recorded","seq":7}),
        json!({"type":"tool_error","seq":9,"kind":"tool_error","message":"unknown"}),
        json!({"type":"start","code":"async () => 1"}),
    ] {
        let mut ownership = ReplyOwnership::default();
        ownership.record_request(&request(json!({"type":"step_begin","seq":7,"name":"s"})));
        ownership.record_reply(&reply(unexpected));
        ownership.record_reply(&reply(json!({"type":"step_decision","seq":7})));
        assert!(!ownership.is_clean());
        assert!(
            ownership.pending.is_empty(),
            "poisoning does not retain requests"
        );
        ownership.reset();
        assert!(
            ownership.is_clean(),
            "only native execution reset clears poison"
        );
    }
    let mut ownership = ReplyOwnership::default();
    ownership.record_request(&request(
        json!({"type":"tool_call","seq":0,"id":"t","params":{}}),
    ));
    let completion = reply(json!({"type":"tool_result","seq":0,"result":null}));
    ownership.record_reply(&completion);
    assert!(ownership.is_clean());
    ownership.record_reply(&completion);
    assert!(
        !ownership.is_clean(),
        "a duplicate ack poisons a previously clean proof"
    );
}

#[test]
fn capacity_exhaustion_is_bounded_and_cannot_requalify_after_later_replies() {
    let mut ownership = ReplyOwnership::default();
    for seq in 0..=MAX_TRACKED_REPLIES as u64 {
        ownership.record_request(&request(
            json!({"type":"tool_call","seq":seq,"id":"t","params":{}}),
        ));
        assert!(ownership.pending.len() <= MAX_TRACKED_REPLIES);
    }
    assert!(ownership.pending.is_empty());
    for seq in 0..=MAX_TRACKED_REPLIES as u64 {
        ownership.record_reply(&reply(
            json!({"type":"tool_result","seq":seq,"result":null}),
        ));
    }
    assert!(!ownership.is_clean());
}

#[test]
fn legacy_done_is_successful_but_has_no_default_reuse_proof() {
    let legacy = request(json!({"type":"done"}));
    assert!(matches!(
        legacy,
        CodeModeRunnerOutput::Done {
            reuse_ready: false,
            ..
        }
    ));
    let qualified = request(json!({"type":"done","reuse_ready":true}));
    assert!(matches!(
        qualified,
        CodeModeRunnerOutput::Done {
            reuse_ready: true,
            ..
        }
    ));
}
