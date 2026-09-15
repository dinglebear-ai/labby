//! Target and backend vocabulary contracts, independent of a replay engine.

use serde::{Deserialize, Serialize};
use serde_json::json;
use verify_core::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Step {
    Increment,
    Reject,
    Fail,
}
struct Target;
impl ScenarioTarget for Target {
    type State = u8;
    type Step = Step;
    fn init(&self, initial: &serde_json::Value) -> Result<u8, ScenarioError> {
        if !initial.is_object() {
            return Err(ScenarioError::InvalidInitial("object required".into()));
        }
        Ok(0)
    }
    fn apply(&self, state: &mut u8, step: &Step) -> Result<StepOutcome, ScenarioError> {
        match step {
            Step::Increment => {
                *state += 1;
                Ok(StepOutcome::Applied {})
            }
            Step::Reject => Ok(StepOutcome::Rejected {
                reason: "not allowed".into(),
            }),
            Step::Fail => Err(ScenarioError::Harness("fixture failure".into())),
        }
    }
    fn check(&self, id: &InvariantId, state: &u8) -> Result<InvariantResult, ScenarioError> {
        if id.as_str() != "TEST-REQ-001" {
            return Err(ScenarioError::UnknownInvariant(id.clone()));
        }
        Ok(if *state > 1 {
            InvariantResult::Violated {
                reason: "duplicate terminal outcome".into(),
            }
        } else {
            InvariantResult::Holds {}
        })
    }
}

#[test]
fn target_distinguishes_observations_rejections_and_harness_errors() {
    let id = "TEST-REQ-001".to_owned().try_into().unwrap();
    let mut state = Target.init(&json!({})).unwrap();
    assert_eq!(
        Target.check(&id, &state).unwrap(),
        InvariantResult::Holds {}
    );
    assert_eq!(
        Target.apply(&mut state, &Step::Reject).unwrap(),
        StepOutcome::Rejected {
            reason: "not allowed".into()
        }
    );
    assert_eq!(state, 0);
    Target.apply(&mut state, &Step::Increment).unwrap();
    Target.apply(&mut state, &Step::Increment).unwrap();
    assert_eq!(
        Target.check(&id, &state).unwrap(),
        InvariantResult::Violated {
            reason: "duplicate terminal outcome".into()
        }
    );
    assert!(matches!(
        Target.apply(&mut state, &Step::Fail),
        Err(ScenarioError::Harness(_))
    ));
    assert!(matches!(
        Target.init(&json!(null)),
        Err(ScenarioError::InvalidInitial(_))
    ));
    let unknown = "TEST-REQ-999".to_owned().try_into().unwrap();
    assert!(matches!(
        Target.check(&unknown, &state),
        Err(ScenarioError::UnknownInvariant(_))
    ));
    assert!(!Target.commutes(&Step::Increment, &Step::Reject));
}

#[test]
fn verdicts_have_distinct_literal_wire_contracts() {
    assert_eq!(
        serde_json::to_value(Verdict::Verified {}).unwrap(),
        json!({"verdict":"verified"})
    );
    assert_eq!(
        serde_json::to_value(Verdict::Bounded {
            bounds: [("depth".into(), json!(5))].into()
        })
        .unwrap(),
        json!({"verdict":"bounded","bounds":{"depth":5}})
    );
    assert_eq!(
        serde_json::to_value(Verdict::Incomplete {
            reason: "deadline".into(),
            explored: Bounds::new()
        })
        .unwrap(),
        json!({"verdict":"incomplete","reason":"deadline","explored":{}})
    );
    assert_eq!(
        serde_json::to_value(InvariantResult::Incomplete {
            reason: "pending obligation".into()
        })
        .unwrap(),
        json!({"result":"incomplete","reason":"pending obligation"})
    );
    assert!(serde_json::from_value::<Verdict>(json!({"verdict":"bounded"})).is_err());
    assert!(
        serde_json::from_value::<Verdict>(json!({"verdict":"verified", "reason":"ignored"}))
            .is_err()
    );
}

#[test]
fn check_plans_require_positive_deadlines() {
    let mut value = json!({"invariant":"TEST-REQ-001", "model":"request", "handle":"terminal",
        "bounds":{}, "seed":7, "timeout_ms":0});
    assert!(serde_json::from_value::<CheckPlan>(value.clone()).is_err());
    value["timeout_ms"] = json!(1000);
    assert_eq!(
        serde_json::from_value::<CheckPlan>(value)
            .unwrap()
            .timeout_ms
            .get(),
        1000
    );
}
