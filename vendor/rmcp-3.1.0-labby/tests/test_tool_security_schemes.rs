use rmcp::model::Tool;

fn tool() -> Tool {
    Tool::new(
        "search",
        "Search documents",
        serde_json::Map::from_iter([(
            "type".to_owned(),
            serde_json::Value::String("object".to_owned()),
        )]),
    )
}

#[test]
fn tool_security_schemes_serialize_with_openai_wire_name_and_shape() {
    let tool = tool().with_security_schemes(vec![
        serde_json::json!({"type": "noauth"}),
        serde_json::json!({"type": "oauth2", "scopes": ["search.read"]}),
    ]);

    let value = serde_json::to_value(tool).expect("tool serializes");
    assert_eq!(
        value["securitySchemes"],
        serde_json::json!([
            {"type": "noauth"},
            {"type": "oauth2", "scopes": ["search.read"]}
        ])
    );
    assert!(value.get("security_schemes").is_none());
}

#[test]
fn tool_security_schemes_deserialize_and_round_trip_extension_objects() {
    let wire = serde_json::json!({
        "name": "search",
        "description": "Search documents",
        "inputSchema": {"type": "object"},
        "securitySchemes": [
            {"type": "oauth2", "scopes": ["search.read"], "x-provider": "example"}
        ]
    });

    let tool: Tool = serde_json::from_value(wire.clone()).expect("tool deserializes");
    assert_eq!(
        tool.security_schemes.as_deref(),
        Some(
            &[serde_json::json!({
                "type": "oauth2",
                "scopes": ["search.read"],
                "x-provider": "example"
            })][..]
        )
    );
    assert_eq!(serde_json::to_value(tool).expect("tool reserializes"), wire);
}

#[test]
fn tool_without_security_schemes_omits_extension_for_standard_mcp_clients() {
    let value = serde_json::to_value(tool()).expect("tool serializes");

    assert!(value.get("securitySchemes").is_none());
}
