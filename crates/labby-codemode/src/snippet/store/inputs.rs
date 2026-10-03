use super::*;

/// Merge caller-supplied snippet inputs with declared defaults and validate types/required fields.
pub fn merge_snippet_input(snippet: &ResolvedSnippet, caller: Value) -> Result<Value, ToolError> {
    let caller = match caller {
        Value::Null => Value::Object(Map::new()),
        Value::Object(map) => Value::Object(map),
        _ => {
            return Err(ToolError::InvalidParam {
                message: "snippet params must be a JSON object".to_string(),
                param: "params".to_string(),
            });
        }
    };

    if snippet.inputs.is_empty() {
        return Ok(caller);
    }

    let caller = caller.as_object().expect("caller normalized to object");
    for key in caller.keys() {
        if !snippet.inputs.contains_key(key) {
            return Err(ToolError::InvalidParam {
                message: format!("unknown snippet input `{key}`"),
                param: format!("params.{key}"),
            });
        }
    }

    let mut merged = Map::new();
    for (name, spec) in &snippet.inputs {
        let value = caller
            .get(name)
            .cloned()
            .or_else(|| spec.default.clone())
            .or_else(|| (!spec.required).then_some(Value::Null));
        let Some(value) = value else {
            return Err(ToolError::MissingParam {
                message: format!("missing required snippet input `{name}`"),
                param: format!("params.{name}"),
            });
        };
        if value.is_null() {
            if !spec.nullable && (caller.contains_key(name) || spec.default.is_some()) {
                return Err(ToolError::InvalidParam {
                    message: format!("snippet input `{name}` does not accept null"),
                    param: format!("params.{name}"),
                });
            }
        } else {
            validate_input_type(name, spec.ty, &value)?;
        }
        merged.insert(name.clone(), value);
    }

    Ok(Value::Object(merged))
}

pub(super) fn parse_inputs_block(
    lines: &[&str],
    mut i: usize,
) -> Result<(BTreeMap<String, SnippetInputSpec>, usize), ToolError> {
    let mut inputs = BTreeMap::new();
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with("  ") {
            break;
        }
        if line.starts_with("    ") {
            return Err(ToolError::InvalidParam {
                message: format!("invalid input declaration line `{}`", line.trim()),
                param: "body".to_string(),
            });
        }
        let Some(input_name) = line.trim().strip_suffix(':') else {
            return Err(ToolError::InvalidParam {
                message: format!("invalid input declaration line `{}`", line.trim()),
                param: "body".to_string(),
            });
        };
        // Input keys are JSON fields, not filesystem-backed snippet slugs.
        // Preserve camelCase callers without relaxing artifact-name safety.
        if input_name.is_empty()
            || input_name.len() > 128
            || !input_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            || inputs.contains_key(input_name)
        {
            return Err(ToolError::InvalidParam {
                message: "snippet input names must be unique, bounded alphanumeric keys".into(),
                param: "body".into(),
            });
        }
        i += 1;

        let mut ty = None;
        let mut required = false;
        let mut nullable = false;
        let mut default = None;
        let mut description = None;
        while i < lines.len() {
            let field_line = lines[i];
            if field_line.trim().is_empty() {
                i += 1;
                continue;
            }
            if !field_line.starts_with("    ") {
                break;
            }
            let Some((key, value)) = field_line.trim().split_once(':') else {
                return Err(ToolError::InvalidParam {
                    message: format!("invalid input field line `{}`", field_line.trim()),
                    param: "body".to_string(),
                });
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "type" => ty = Some(parse_input_type(value)?),
                "required" => required = parse_bool(value, "required")?,
                "nullable" => nullable = parse_bool(value, "nullable")?,
                "default" => {
                    default = Some(parse_default_value(
                        field_line
                            .split_once(':')
                            .expect("field delimiter parsed")
                            .1
                            .trim(),
                    ))
                }
                "description" => description = Some(value.to_string()),
                _ => {}
            }
            i += 1;
        }

        let ty = ty.ok_or_else(|| ToolError::InvalidParam {
            message: format!("snippet input `{input_name}` requires `type`"),
            param: "body".to_string(),
        })?;
        if let Some(default_value) = &default {
            if default_value.is_null() {
                if !nullable {
                    return Err(ToolError::InvalidParam {
                        message: format!(
                            "snippet input `{input_name}` null default requires nullable: true"
                        ),
                        param: "body".into(),
                    });
                }
            } else {
                validate_input_type(input_name, ty, default_value)?;
            }
        }
        inputs.insert(
            input_name.to_string(),
            SnippetInputSpec {
                ty,
                required,
                nullable,
                default,
                description,
            },
        );
    }
    Ok((inputs, i))
}

fn parse_input_type(value: &str) -> Result<SnippetInputType, ToolError> {
    match value {
        "string" => Ok(SnippetInputType::String),
        "integer" => Ok(SnippetInputType::Integer),
        "number" => Ok(SnippetInputType::Number),
        "boolean" => Ok(SnippetInputType::Boolean),
        "object" => Ok(SnippetInputType::Object),
        "array" => Ok(SnippetInputType::Array),
        "json" => Ok(SnippetInputType::Json),
        _ => Err(ToolError::InvalidParam {
            message: format!("unsupported snippet input type `{value}`"),
            param: "body".to_string(),
        }),
    }
}

fn parse_bool(value: &str, field: &str) -> Result<bool, ToolError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ToolError::InvalidParam {
            message: format!("frontmatter `{field}` must be true or false"),
            param: "body".to_string(),
        }),
    }
}

fn parse_default_value(value: &str) -> Value {
    let value = value.trim();
    // JSON-quoted strings must not become numbers, booleans or lose escapes.
    if let Ok(json) = serde_json::from_str::<Value>(value) {
        return json;
    }
    if value == "null" {
        return Value::Null;
    }
    if value.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(n) = value.parse::<i64>()
        && n.to_string() == value
    {
        return Value::Number(n.into());
    }
    if let Ok(n) = value.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
        && n.to_string() == value
    {
        return Value::Number(num);
    }
    if let Ok(json) = serde_json::from_str::<Value>(value)
        && (json.is_object() || json.is_array())
    {
        return json;
    }
    Value::String(value.trim_matches('"').to_string())
}

pub(super) fn validate_input_type(
    name: &str,
    ty: SnippetInputType,
    value: &Value,
) -> Result<(), ToolError> {
    let ok = match ty {
        SnippetInputType::String => value.is_string(),
        SnippetInputType::Integer => value.as_i64().is_some(),
        SnippetInputType::Number => value.is_number(),
        SnippetInputType::Boolean => value.is_boolean(),
        SnippetInputType::Object => value.is_object(),
        SnippetInputType::Array => value.is_array(),
        SnippetInputType::Json => true,
    };
    if ok {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        message: format!("snippet input `{name}` has wrong type; expected {ty:?}"),
        param: format!("params.{name}"),
    })
}
