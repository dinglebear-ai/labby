use rmcp::model::{
    CallToolRequestParams, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationSchema, InputRequest, InputRequests, InputRequiredResult, PrimitiveSchemaDefinition,
};
use serde_json::Value;

pub(crate) const DESTRUCTIVE_CONFIRMATION_INPUT: &str = "destructive_confirmation";

pub(crate) enum DestructiveConfirmation {
    Proceed,
    InputRequired(InputRequiredResult),
    Refused,
}

/// Apply the 2026-07-28 MRTR elicitation pattern to one destructive tool call.
///
/// The original tool request already identifies the operation, so this carries
/// no custom `requestState`: the client simply retries that request with the
/// elicitation result in `inputResponses`.
pub(crate) fn destructive_confirmation(
    request: &CallToolRequestParams,
    service: &str,
    action: &str,
) -> DestructiveConfirmation {
    let supports_form_elicitation = request
        .meta
        .as_ref()
        .and_then(|meta| meta.client_capabilities())
        .and_then(|capabilities| capabilities.elicitation)
        .and_then(|elicitation| elicitation.form)
        .is_some();
    if !supports_form_elicitation {
        return DestructiveConfirmation::Refused;
    }

    if let Some(responses) = request.input_responses.as_ref() {
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
        return DestructiveConfirmation::Refused;
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
    DestructiveConfirmation::InputRequired(InputRequiredResult::from_input_requests(requests))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rmcp::model::{
        CallToolRequestParams, ClientCapabilities, ElicitationCapability,
        FormElicitationCapability, Implementation, ProtocolVersion, RequestMetaObject,
    };
    use serde_json::json;

    use super::{DestructiveConfirmation, destructive_confirmation};

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
    fn destructive_confirmation_uses_mrtr_without_request_state() {
        let request = elicitation_request();

        let DestructiveConfirmation::InputRequired(result) =
            destructive_confirmation(&request, "danger", "danger.delete")
        else {
            panic!("expected input_required");
        };

        assert!(result.request_state.is_none());
        let requests = result.input_requests.expect("inputRequests");
        assert_eq!(requests.len(), 1);
        assert!(requests.contains_key("destructive_confirmation"));
    }

    #[test]
    fn destructive_confirmation_accepts_the_retried_elicitation_response() {
        let mut request = elicitation_request();
        request.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".to_string(),
            json!({"action": "accept", "content": {"confirm": true}}),
        )]));

        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete"),
            DestructiveConfirmation::Proceed
        ));
    }

    #[test]
    fn destructive_confirmation_fails_closed_without_elicitation() {
        let request = CallToolRequestParams::new("danger");

        assert!(matches!(
            destructive_confirmation(&request, "danger", "danger.delete"),
            DestructiveConfirmation::Refused
        ));
    }
}
