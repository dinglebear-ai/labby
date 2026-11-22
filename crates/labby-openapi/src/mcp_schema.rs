//! A deliberately small MCP input projection for the existing flat dispatcher.
//! This does not change which operations the Code Mode dispatcher accepts.
//! Refuse input constraints or body presence rules the flat MCP contract
//! cannot express rather than advertising a weaker or unsatisfiable schema.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
pub(crate) struct McpDescriptor {
    pub(crate) description: String,
    pub(crate) input: Map<String, Value>,
}

pub(crate) type Tools = HashMap<String, Option<McpDescriptor>>;

// Only descriptive metadata and explicitly handled assertions may be omitted.
fn supports_schema_keys(schema: &Map<String, Value>, supported: &[&str]) -> bool {
    schema.iter().all(|(key, value)| {
        supported.contains(&key.as_str())
            || matches!(
                key.as_str(),
                "title"
                    | "description"
                    | "example"
                    | "examples"
                    | "default"
                    | "deprecated"
                    | "$comment"
            )
            || (key == "nullable" && value == &Value::Bool(false))
    })
}

fn scalar_type(schema: Option<&Value>) -> Option<&str> {
    let schema = schema?.as_object()?;
    // The standalone handler validates scalar types but not other assertions
    // such as enums, numeric bounds, or patterns.
    if !supports_schema_keys(schema, &["type"]) {
        return None;
    }
    match schema.get("type")?.as_str()? {
        kind @ ("string" | "boolean" | "integer" | "number") => Some(kind),
        _ => None,
    }
}

// The dispatcher expands braces anywhere in a segment. Repeated placeholders
// consume the same input; an MCP object declares that input only once.
fn path_parameters(path: &str) -> Option<BTreeSet<&str>> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '?', '#']) {
        return None;
    }
    let mut names = BTreeSet::new();
    let mut open = None;
    for (index, ch) in path.char_indices() {
        match ch {
            '{' => {
                if open.replace(index + 1).is_some() {
                    return None;
                }
            }
            '}' => {
                let name = &path[open.take()?..index];
                if name.is_empty()
                    || name
                        .chars()
                        .any(|ch| ch.is_whitespace() || ch.is_control() || ch == '/')
                {
                    return None;
                }
                names.insert(name);
            }
            _ => {}
        }
    }
    open.is_none().then_some(names)
}

fn parameter_identity(parameter: &Value) -> Option<(&str, &str)> {
    let kind = parameter.get("in")?.as_str()?;
    let name = parameter.get("name")?.as_str()?;
    (!name.is_empty()).then_some((kind, name))
}

fn add_parameter(
    parameter: &Value,
    uses_body: bool,
    paths: &BTreeSet<&str>,
    properties: &mut Map<String, Value>,
    required: &mut BTreeSet<String>,
) -> Option<()> {
    let (kind, name) = parameter_identity(parameter)?;
    let is_required = match parameter.get("required") {
        Some(Value::Bool(required)) => *required,
        None => false,
        Some(_) => return None,
    };
    // The native dispatcher cannot send a per-call header/cookie, or send
    // both query and body values on the same write request.
    let style = match kind {
        "path" if paths.contains(name) => "simple",
        "query" if !uses_body && !paths.contains(name) => "form",
        "query" if uses_body => return (!is_required).then_some(()),
        "header" | "cookie" => return (!is_required).then_some(()),
        _ => return None,
    };
    if parameter.get("content").is_some()
        || parameter
            .get("style")
            .is_some_and(|value| value.as_str() != Some(style))
        || parameter
            .get("allowReserved")
            .is_some_and(|value| value != &Value::Bool(false))
    {
        return None;
    }
    let scalar = scalar_type(parameter.get("schema"))?;
    if properties
        .insert(name.to_owned(), json!({ "type": scalar }))
        .is_some()
    {
        return None;
    }
    if kind == "path" || is_required {
        required.insert(name.to_owned());
    }
    Some(())
}

fn add_body(
    body: &Value,
    properties: &mut Map<String, Value>,
    required: &mut BTreeSet<String>,
) -> Option<()> {
    if body.get("$ref").is_some() {
        return None;
    }
    let body_required = match body.get("required") {
        Some(Value::Bool(required)) => *required,
        None => false,
        Some(_) => return None,
    };
    let schema = body
        .pointer("/content/application~1json/schema")?
        .as_object()?;
    if schema.get("type")?.as_str()? != "object"
        || !supports_schema_keys(
            schema,
            &["type", "properties", "required", "additionalProperties"],
        )
        || schema
            .get("additionalProperties")
            .is_some_and(|value| value != &Value::Bool(false))
    {
        return None;
    }
    let fields = schema.get("properties")?.as_object()?;
    if fields.is_empty() {
        return None;
    }

    let required_fields = match schema.get("required") {
        Some(required) => required.as_array()?.as_slice(),
        None => &[],
    };
    // apply_body omits JSON when no non-path argument remains. A required
    // body therefore needs a required field; an optional body with required
    // fields would need conditional validation the scalar handler cannot do.
    if body_required == required_fields.is_empty() {
        return None;
    }
    for name in required_fields {
        let name = name.as_str()?;
        if !fields.contains_key(name) {
            return None;
        }
        required.insert(name.to_owned());
    }
    for (name, value) in fields {
        if properties.contains_key(name) {
            return None;
        }
        let scalar = scalar_type(Some(value))?;
        properties.insert(name.clone(), json!({"type": scalar}));
    }
    Some(())
}

pub(crate) fn describe(
    method: &reqwest::Method,
    path: &str,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
) -> Option<McpDescriptor> {
    let uses_body = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
    if !matches!(
        method.as_str(),
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE"
    ) {
        return None;
    }
    let paths = path_parameters(path)?;
    // OpenAPI operation parameters override inherited parameters by (in, name).
    // Resolve that identity first, then check the restricted flat projection.
    let mut parameters = BTreeMap::new();
    for item in [path_item, operation] {
        if let Some(params) = item.get("parameters") {
            let mut seen = BTreeSet::new();
            for parameter in params.as_array()? {
                let key = parameter_identity(parameter)?;
                if !seen.insert(key) {
                    return None;
                }
                parameters.insert(key, parameter);
            }
        }
    }

    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    for parameter in parameters.into_values() {
        add_parameter(parameter, uses_body, &paths, &mut properties, &mut required)?;
    }
    for param in paths {
        properties
            .entry(param.to_owned())
            .or_insert_with(|| json!({"type":"string"}));
        required.insert(param.to_owned());
    }
    if let Some(body) = operation.get("requestBody") {
        if !uses_body {
            return None;
        }
        add_body(body, &mut properties, &mut required)?;
    }
    let description = operation
        .get("summary")
        .or_else(|| operation.get("description"))
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{method} {path}"),
            |text| text.chars().take(240).collect(),
        );
    let input = Map::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), Value::Object(properties)),
        ("required".into(), json!(required)),
        ("additionalProperties".into(), json!(false)),
    ]);
    Some(McpDescriptor { description, input })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_path_query_and_body_to_the_dispatchers_flat_shape() {
        let path = json!({"parameters": [{"in":"path","name":"id","schema":{"type":"integer"}}]});
        let get = json!({"summary":"Get an item","parameters":[{"in":"query","name":"verbose","schema":{"type":"boolean"}}]});
        let read = describe(
            &reqwest::Method::GET,
            "/items/{id}",
            path.as_object().unwrap(),
            get.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(read.input["properties"]["id"]["type"], "integer");
        assert_eq!(read.input["properties"]["verbose"]["type"], "boolean");
        assert_eq!(read.input["required"], json!(["id"]));
        let post = json!({"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}}}}});
        let write = describe(
            &reqwest::Method::POST,
            "/items/{id}",
            path.as_object().unwrap(),
            post.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(write.input["required"], json!(["id", "title"]));
    }

    #[test]
    fn refuses_metadata_that_the_dispatcher_cannot_represent() {
        let empty = Map::new();
        let unresolved = json!({"parameters":[{"$ref":"#/components/parameters/key"}]});
        assert!(
            describe(
                &reqwest::Method::GET,
                "/items",
                &empty,
                unresolved.as_object().unwrap()
            )
            .is_none()
        );
        let required_header =
            json!({"parameters":[{"in":"header","name":"Authorization","required":true}]});
        assert!(
            describe(
                &reqwest::Method::GET,
                "/items",
                &empty,
                required_header.as_object().unwrap()
            )
            .is_none()
        );
    }

    fn descriptor(
        method: reqwest::Method,
        path: &str,
        path_item: Value,
        operation: Value,
    ) -> Option<McpDescriptor> {
        describe(
            &method,
            path,
            path_item.as_object().unwrap(),
            operation.as_object().unwrap(),
        )
    }

    #[test]
    fn refuses_scalar_validation_it_cannot_publish_or_enforce() {
        for schema in [
            json!({"type":"string","enum":["public","private"]}),
            json!({"type":"integer","minimum":1}),
            json!({"type":"string","pattern":"^[a-z]+$"}),
        ] {
            let operation = json!({
                "parameters": [{"in":"query","name":"filter","schema":schema}]
            });
            assert!(descriptor(reqwest::Method::GET, "/items", json!({}), operation).is_none());
        }
        let operation = json!({
            "requestBody": {"required":true,"content":{"application/json":{"schema":{
                "type":"object",
                "properties":{"status":{"type":"string","enum":["draft","published"]}},
                "required":["status"]
            }}}}
        });
        assert!(descriptor(reqwest::Method::POST, "/items", json!({}), operation).is_none());
    }

    #[test]
    fn resolves_operation_parameter_override_before_projection() {
        let path_item = json!({
            "parameters":[{"in":"query","name":"filter","required":true,
                           "schema":{"type":"integer","minimum":1}}]
        });
        let operation = json!({
            "parameters":[{"in":"query","name":"filter",
                           "schema":{"type":"string","description":"Filter text"}}]
        });
        let projected = descriptor(reqwest::Method::GET, "/items", path_item, operation).unwrap();
        assert_eq!(
            projected.input["properties"]["filter"],
            json!({"type":"string"})
        );
        assert_eq!(projected.input["required"], json!([]));
    }

    #[test]
    fn refuses_path_query_name_collision_across_parameter_levels() {
        let path_item = json!({
            "parameters":[{"in":"path","name":"id","required":true,"schema":{"type":"string"}}]
        });
        let operation = json!({
            "parameters":[{"in":"query","name":"id","required":true,"schema":{"type":"integer"}}]
        });
        // Includes a path placeholder without a declared parameter: this
        // projection infers a string but may not reuse a query input for it.
        for item in [path_item, json!({})] {
            assert!(
                descriptor(reqwest::Method::GET, "/items/{id}", item, operation.clone()).is_none()
            );
        }
    }

    #[test]
    fn refuses_path_parameter_serialization_the_dispatcher_does_not_implement() {
        let operation = json!({
            "parameters":[{"in":"path","name":"id","required":true,
                           "style":"matrix","schema":{"type":"string"}}]
        });
        assert!(descriptor(reqwest::Method::GET, "/items/{id}", json!({}), operation).is_none());
    }

    #[test]
    fn admits_only_body_presence_expressible_with_flat_arguments() {
        for (body_flag, required_properties, expected_required) in [
            (Some(true), json!(["title"]), Some(json!(["title"]))),
            (Some(false), json!([]), Some(json!([]))),
            (None, json!([]), Some(json!([]))),
            (Some(true), json!([]), None),
            (Some(false), json!(["title"]), None),
            (None, json!(["title"]), None),
        ] {
            let mut body = json!({"content":{"application/json":{"schema":{
                "type":"object",
                "properties":{"title":{"type":"string"}},
                "required":required_properties
            }}}});
            if let Some(required) = body_flag {
                body["required"] = json!(required);
            }
            let projected = descriptor(
                reqwest::Method::POST,
                "/items",
                json!({}),
                json!({"requestBody":body}),
            );
            assert_eq!(
                projected.map(|value| value.input["required"].clone()),
                expected_required,
                "requestBody.required = {body_flag:?}, fields = {required_properties}",
            );
        }
    }

    #[test]
    fn a_path_input_cannot_satisfy_a_required_body_property() {
        let operation = json!({
            "requestBody":{"required":true,"content":{"application/json":{"schema":{
                "type":"object",
                "properties":{"title":{"type":"string"}},
                "required":["id"]
            }}}}
        });
        assert!(descriptor(reqwest::Method::POST, "/items/{id}", json!({}), operation).is_none());
    }

    #[test]
    fn projects_embedded_and_repeated_path_parameters_once() {
        let path = "/v{version}/items/{item.id}.json/related/{item.id}-{version}";
        let path_item = json!({
            "parameters":[
                {"in":"path","name":"version","required":true,"schema":{"type":"integer"}},
                {"in":"path","name":"item.id","required":true,"schema":{"type":"string"}}
            ]
        });
        let projected =
            descriptor(reqwest::Method::GET, path, path_item.clone(), json!({})).unwrap();
        assert_eq!(
            projected.input["properties"],
            json!({"version":{"type":"integer"},"item.id":{"type":"string"}})
        );
        assert_eq!(projected.input["required"], json!(["item.id", "version"]));

        let collision = json!({
            "parameters":[{"in":"query","name":"item.id","schema":{"type":"string"}}]
        });
        assert!(descriptor(reqwest::Method::GET, path, path_item, collision).is_none());
    }

    #[test]
    fn refuses_malformed_path_templates() {
        for path in [
            "/items/{id",
            "/items/id}",
            "/items/{id}}",
            "/items/{}",
            "/items/{{id}}",
            "/items/{id name}",
            "/items/{id/name}",
            "/items/{id\tname}",
            "//host/items/{id}",
            "/items/{id}?query=1",
            "items/{id}",
        ] {
            assert!(
                descriptor(reqwest::Method::GET, path, json!({}), json!({})).is_none(),
                "{path:?} is not a supported path template"
            );
        }
    }
}
