//! Syntactic normalization behavior.

use serde_json::{Value, json};
use verify_scenario::normalize::{Commutes, NeverCommutes, normalize_identifiers_only};
use verify_scenario::{Scenario, normalize_syntactic};

fn scenario_with(steps: &Value, initial: Option<&Value>) -> Scenario {
    let initial_fragment = initial
        .map(|value| format!(",\n  \"initial\": {value}"))
        .unwrap_or_default();
    let text = format!(
        r#"{{
  "schema": 1,
  "project": "labby",
  "model": "gateway",
  "invariant": "LABBY-REQ-001",
  "origin": {{ "kind": "stateright" }},
  "steps": {steps},
  "expect": "invariant_violated"{initial_fragment}
}}"#
    );
    Scenario::parse(&text).expect("parse")
}

#[test]
fn identifiers_are_renumbered_in_order_of_first_appearance() {
    let a = scenario_with(
        &json!([{ "on": "upstream_7" }, { "on": "upstream_2" }, { "on": "upstream_7" }]),
        None,
    );
    let b = scenario_with(
        &json!([{ "on": "upstream_1" }, { "on": "upstream_0" }, { "on": "upstream_1" }]),
        None,
    );
    let na = normalize_identifiers_only(&a);
    let nb = normalize_identifiers_only(&b);
    assert_eq!(
        na.steps, nb.steps,
        "traces differing only in id choice must collapse"
    );
    assert_eq!(na.steps[0]["on"], json!("upstream_0"));
    assert_eq!(na.steps[1]["on"], json!("upstream_1"));
    // The repeat must map back to the same canonical id, not a fresh one.
    assert_eq!(na.steps[2]["on"], json!("upstream_0"));
}

#[test]
fn distinct_prefixes_are_numbered_independently() {
    let scenario = scenario_with(&json!([{ "a": "req_5", "b": "conn_9" }]), None);
    let out = normalize_identifiers_only(&scenario);
    assert_eq!(out.steps[0]["a"], json!("req_0"));
    assert_eq!(out.steps[0]["b"], json!("conn_0"));
}

#[test]
fn non_identifier_strings_are_left_alone() {
    // Renaming anything that merely contains an underscore would mangle
    // payloads that happen to look like ids.
    let scenario = scenario_with(
        &json!([{ "note": "cancelled_by_peer", "path": "/a/b", "id": "x_12" }]),
        None,
    );
    let out = normalize_identifiers_only(&scenario);
    assert_eq!(out.steps[0]["note"], json!("cancelled_by_peer"));
    assert_eq!(out.steps[0]["path"], json!("/a/b"));
    assert_eq!(out.steps[0]["id"], json!("x_0"));
}

#[test]
fn initial_state_identifiers_are_renamed_consistently_with_steps() {
    let scenario = scenario_with(
        &json!([{ "on": "upstream_4" }]),
        Some(&json!({ "primary": "upstream_4" })),
    );
    let out = normalize_identifiers_only(&scenario);
    let initial = out.initial.clone().expect("initial present");
    assert_eq!(initial["primary"], json!("upstream_0"));
    assert_eq!(out.steps[0]["on"], json!("upstream_0"));
}

#[test]
fn nothing_is_reordered_by_default() {
    let scenario = scenario_with(&json!([{ "e": "zulu" }, { "e": "alpha" }]), None);
    let out = normalize_syntactic(&scenario, &NeverCommutes);
    assert_eq!(out.steps[0]["e"], json!("zulu"), "default must not reorder");
}

struct AllCommute;
impl Commutes for AllCommute {
    fn commutes(&self, _a: &Value, _b: &Value) -> bool {
        true
    }
}

#[test]
fn declared_commuting_steps_reach_a_canonical_order() {
    let a = scenario_with(&json!([{ "e": "zulu" }, { "e": "alpha" }]), None);
    let b = scenario_with(&json!([{ "e": "alpha" }, { "e": "zulu" }]), None);
    let na = normalize_syntactic(&a, &AllCommute);
    let nb = normalize_syntactic(&b, &AllCommute);
    assert_eq!(na.steps, nb.steps, "both orderings must reach one form");
}

struct OnlyFirstPairCommutes;
impl Commutes for OnlyFirstPairCommutes {
    fn commutes(&self, a: &Value, b: &Value) -> bool {
        let names = [a["e"].as_str(), b["e"].as_str()];
        names.contains(&Some("zulu")) && names.contains(&Some("mid"))
    }
}

#[test]
fn reordering_never_crosses_a_non_commuting_step() {
    // "alpha" sorts first but must not migrate past "mid", which does not
    // commute with it. A naive sort would produce alpha, mid, zulu.
    let scenario = scenario_with(
        &json!([{ "e": "zulu" }, { "e": "mid" }, { "e": "alpha" }]),
        None,
    );
    let out = normalize_syntactic(&scenario, &OnlyFirstPairCommutes);
    assert_eq!(out.steps[2]["e"], json!("alpha"), "alpha must stay last");
}

#[test]
fn normalization_does_not_mutate_its_input() {
    // A caller comparing pre- and post-normalization verdicts needs both.
    let scenario = scenario_with(&json!([{ "on": "upstream_7" }]), None);
    let before = scenario.clone();
    let _ = normalize_identifiers_only(&scenario);
    assert_eq!(scenario, before);
}
