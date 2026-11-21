//! A deliberately small MCP input projection for the existing flat dispatcher.
//! This does not change which operations the Code Mode dispatcher accepts.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
pub(crate) struct McpDescriptor {
    pub(crate) description: String,
    pub(crate) input: Map<String, Value>,
}

pub(crate) type Tools = HashMap<String, Option<McpDescriptor>>;

fn scalar_type(schema: Option<&Value>) -> Option<&str> {
    let schema = schema?;
    if schema.get("$ref").is_some() {
        return None;
    }
    match schema.get("type")?.as_str()? {
        kind @ ("string" | "boolean" | "integer" | "number") => Some(kind),
        _ => None,
    }
}

fn path_parameters(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter_map(|part| part.strip_prefix('{')?.strip_suffix('}'))
}

fn add_parameter(
    parameter: &Value,
    uses_body: bool,
    properties: &mut Map<String, Value>,
    required: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> Option<()> {
    let kind = parameter.get("in")?.as_str()?;
    let name = parameter.get("name")?.as_str()?;
    // The native dispatcher cannot send a per-call header/cookie, or send
    // both query and body values on the same write request.
    if kind != "path" && (kind != "query" || uses_body) {
        return (parameter.get("required").and_then(Value::as_bool) != Some(true)).then_some(());
    }
    if !seen.insert(name.to_owned()) {
        return None;
    }
    let scalar = scalar_type(parameter.get("schema"))?;
    properties.insert(name.to_owned(), json!({ "type": scalar }));
    if kind == "path" || parameter.get("required").and_then(Value::as_bool) == Some(true) {
        required.insert(name.to_owned());
    } else {
        required.remove(name);
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
    // The existing dispatcher only substitutes standalone path segments.
    let paths: BTreeSet<_> = path_parameters(path).collect();
    if path.matches('{').count() != paths.len() || path.matches('}').count() != paths.len() {
        return None;
    }
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    for item in [path_item, operation] {
        if let Some(params) = item.get("parameters") {
            let mut seen = BTreeSet::new();
            for parameter in params.as_array()? {
                add_parameter(
                    parameter,
                    uses_body,
                    &mut properties,
                    &mut required,
                    &mut seen,
                )?;
            }
        }
    }
    for param in paths {
        properties
            .entry(param.to_owned())
            .or_insert_with(|| json!({"type":"string"}));
        required.insert(param.to_owned());
    }
    if let Some(body) = operation.get("requestBody") {
        if !uses_body || body.get("$ref").is_some() {
            return None;
        }
        let schema = body.pointer("/content/application~1json/schema")?;
        if schema.get("$ref").is_some() || schema.get("type")?.as_str()? != "object" {
            return None;
        }
        if let Some(fields) = schema.get("properties") {
            for (name, value) in fields.as_object()? {
                if properties.contains_key(name) {
                    return None;
                }
                let scalar = scalar_type(Some(value))?;
                properties.insert(name.clone(), json!({"type": scalar}));
            }
        }
        if let Some(fields) = schema.get("required") {
            for name in fields.as_array()? {
                let name = name.as_str()?;
                if !properties.contains_key(name) {
                    return None;
                }
                required.insert(name.to_owned());
            }
        }
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
        let post = json!({"requestBody":{"content":{"application/json":{"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}}}}});
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
}
