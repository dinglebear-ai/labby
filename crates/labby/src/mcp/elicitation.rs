use rmcp::model::{
    CallToolRequestParams, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationSchema, InputRequest, InputRequests, InputRequiredResult, PrimitiveSchemaDefinition,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const DESTRUCTIVE_CONFIRMATION_INPUT: &str = "destructive_confirmation";
const CONFIRMATION_TTL: Duration = Duration::from_mins(2);
const MAX_CONFIRMATIONS: usize = 256;
const MAX_CONFIRMATIONS_PER_OWNER: usize = 8;

struct PendingConfirmation {
    binding: String,
    owner: String,
    expires: Instant,
}

fn pending_confirmations() -> &'static Mutex<HashMap<String, PendingConfirmation>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingConfirmation>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) enum DestructiveConfirmation {
    Proceed,
    InputRequired(InputRequiredResult),
    Refused,
}

/// Apply the 2026-07-28 MRTR elicitation pattern to one destructive tool call.
///
/// The server issues opaque, expiring `requestState` bound to the complete
/// authorization and request context supplied by the caller. A retry consumes
/// that state before validating the response, so mismatches and replays fail
/// closed and cannot execute the destructive operation.
pub(crate) fn destructive_confirmation(
    request: &CallToolRequestParams,
    service: &str,
    action: &str,
    binding: &str,
    owner: &str,
) -> DestructiveConfirmation {
    let supports_form_elicitation = request
        .meta
        .as_ref()
        .and_then(|meta| meta.client_capabilities())
        .and_then(|capabilities| capabilities.elicitation)
        .and_then(|elicitation| elicitation.form)
        .is_some();
    if !supports_form_elicitation {
        return DestructiveConfirmation::Proceed;
    }

    if let Some(responses) = request.input_responses.as_ref() {
        let Some(state) = request.request_state.as_deref() else {
            return DestructiveConfirmation::Refused;
        };
        let pending = pending_confirmations()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(state);
        let Some(pending) = pending else {
            return DestructiveConfirmation::Refused;
        };
        if pending.expires <= Instant::now() || pending.binding != binding {
            return DestructiveConfirmation::Refused;
        }
        let accepted = responses
            .get(DESTRUCTIVE_CONFIRMATION_INPUT)
            .and_then(|value| serde_json::from_value::<ElicitResult>(value.clone()).ok())
            .is_some_and(|result| {
                result.action == ElicitationAction::Accept
                    && result
                        .content
                        .as_ref()
                        .and_then(|content| content.get("confirm"))
                        .and_then(Value::as_bool)
                        == Some(true)
            });
        return if accepted {
            DestructiveConfirmation::Proceed
        } else {
            DestructiveConfirmation::Refused
        };
    }

    let Ok(schema) = ElicitationSchema::builder()
        .required_property(
            "confirm",
            PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::default()),
        )
        .build()
    else {
        return DestructiveConfirmation::Proceed;
    };
    let params = ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: format!(
            "Action `{service}.{action}` is destructive and cannot be undone. \
             Set `confirm` to true to proceed."
        ),
        requested_schema: schema,
    };
    let requests = InputRequests::from([(
        DESTRUCTIVE_CONFIRMATION_INPUT.to_string(),
        InputRequest::Elicitation(ElicitRequest::new(params)),
    )]);
    let state = ulid::Ulid::new().to_string();
    let now = Instant::now();
    let mut pending = pending_confirmations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pending.retain(|_, value| value.expires > now);
    let owner_count = pending
        .values()
        .filter(|value| value.owner == owner)
        .count();
    if pending.len() >= MAX_CONFIRMATIONS || owner_count >= MAX_CONFIRMATIONS_PER_OWNER {
        return DestructiveConfirmation::Refused;
    }
    pending.insert(
        state.clone(),
        PendingConfirmation {
            binding: binding.to_owned(),
            owner: owner.to_owned(),
            expires: now + CONFIRMATION_TTL,
        },
    );
    let mut result = InputRequiredResult::from_input_requests(requests);
    result.request_state = Some(state);
    DestructiveConfirmation::InputRequired(result)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rmcp::model::{
        CallToolRequestParams, ClientCapabilities, ElicitationCapability,
        FormElicitationCapability, Implementation, ProtocolVersion, RequestMetaObject,
    };
    use serde_json::json;

    use super::{DestructiveConfirmation, MAX_CONFIRMATIONS_PER_OWNER, destructive_confirmation};

    fn elicitation_request() -> CallToolRequestParams {
        let capabilities = ClientCapabilities::builder()
            .enable_elicitation_with(
                ElicitationCapability::new().with_form(FormElicitationCapability::new()),
            )
            .build();
        let mut request = CallToolRequestParams::new("danger");
        request.meta = Some(RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("test-client", "1.0.0"),
            capabilities,
        ));
        request
    }

    #[test]
    fn destructive_confirmation_uses_server_owned_request_state() {
        let request = elicitation_request();

        let DestructiveConfirmation::InputRequired(result) =
            destructive_confirmation(&request, "danger", "danger.delete", "binding-a", "owner-a")
        else {
            panic!("expected input_required");
        };

        assert!(result.request_state.is_some());
        let requests = result.input_requests.expect("inputRequests");
        assert_eq!(requests.len(), 1);
        assert!(requests.contains_key("destructive_confirmation"));
    }

    #[test]
    fn destructive_confirmation_accepts_the_retried_elicitation_response() {
        let mut request = elicitation_request();
        let DestructiveConfirmation::InputRequired(challenge) =
            destructive_confirmation(&request, "danger", "danger.delete", "binding-b", "owner-b")
        else {
            panic!("expected input_required");
        };
        request.request_state = challenge.request_state;
        request.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".to_string(),
            json!({"action": "accept", "content": {"confirm": true}}),
        )]));

        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete", "binding-b", "owner-b"),
            DestructiveConfirmation::Proceed
        ));
        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete", "binding-b", "owner-b"),
            DestructiveConfirmation::Refused
        ));
    }

    #[test]
    fn confirmation_capacity_is_partitioned_per_owner() {
        let request = elicitation_request();
        let owner = format!("quota-owner-{}", ulid::Ulid::new());
        let mut states = Vec::new();
        for index in 0..MAX_CONFIRMATIONS_PER_OWNER {
            let DestructiveConfirmation::InputRequired(challenge) = destructive_confirmation(
                &request,
                "danger",
                "danger.delete",
                &format!("binding-{index}"),
                &owner,
            ) else {
                panic!("owner quota rejected too early");
            };
            states.push((challenge.request_state.unwrap(), format!("binding-{index}")));
        }
        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete", "overflow", &owner),
            DestructiveConfirmation::Refused
        ));
        let other_owner = format!("other-owner-{}", ulid::Ulid::new());
        assert!(matches!(
            destructive_confirmation(
                &request,
                "danger",
                "danger.delete",
                "other-binding",
                &other_owner
            ),
            DestructiveConfirmation::InputRequired(_)
        ));

        for (state, binding) in states {
            let mut response = elicitation_request();
            response.request_state = Some(state);
            response.input_responses = Some(BTreeMap::from([(
                "destructive_confirmation".to_string(),
                json!({"action": "decline"}),
            )]));
            assert!(matches!(
                destructive_confirmation(&response, "danger", "danger.delete", &binding, &owner),
                DestructiveConfirmation::Refused
            ));
        }
    }

    #[test]
    fn destructive_confirmation_burns_state_on_binding_mismatch() {
        let mut request = elicitation_request();
        let DestructiveConfirmation::InputRequired(challenge) = destructive_confirmation(
            &request,
            "danger",
            "danger.delete",
            "binding-original",
            "owner-c",
        ) else {
            panic!("expected input_required");
        };
        request.request_state = challenge.request_state;
        request.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".to_string(),
            json!({"action": "accept", "content": {"confirm": true}}),
        )]));
        assert!(matches!(
            destructive_confirmation(
                &request,
                "danger",
                "danger.delete",
                "binding-changed",
                "owner-c"
            ),
            DestructiveConfirmation::Refused
        ));
        assert!(matches!(
            destructive_confirmation(
                &request,
                "danger",
                "danger.delete",
                "binding-original",
                "owner-c"
            ),
            DestructiveConfirmation::Refused
        ));
    }

    #[test]
    fn destructive_confirmation_does_not_gate_clients_without_elicitation() {
        let request = CallToolRequestParams::new("danger");

        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete", "binding-c", "owner-d"),
            DestructiveConfirmation::Proceed
        ));
    }
}
