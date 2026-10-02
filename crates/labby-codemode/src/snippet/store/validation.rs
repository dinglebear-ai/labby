use super::*;

pub(super) fn snippet_metadata_fields(
    metadata: Option<SnippetFrontmatter>,
) -> (
    Option<String>,
    Vec<String>,
    BTreeMap<String, SnippetInputSpec>,
    Option<SnippetToolDeclarations>,
) {
    metadata
        .map(|metadata| {
            (
                Some(metadata.description),
                metadata.tags,
                metadata.inputs,
                metadata.tools,
            )
        })
        .unwrap_or_default()
}

pub(super) fn validate_snippet_body_structure(name: &str, body: &str) -> Result<(), ToolError> {
    if body.len() > MAX_SNIPPET_FILE_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!("snippet file exceeds {MAX_SNIPPET_FILE_BYTES} bytes"),
            param: "body".to_string(),
        });
    }
    if let Some(metadata) = frontmatter(body)? {
        if metadata.name != name {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "frontmatter name `{}` does not match snippet name `{name}`",
                    metadata.name
                ),
                param: "name".to_string(),
            });
        }
    }
    let code = if has_frontmatter(body) || body.contains("```") {
        extract_javascript_block(body)?
    } else {
        body.trim().to_string()
    };
    if code.len() > MAX_SNIPPET_CODE_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!("snippet code exceeds {MAX_SNIPPET_CODE_BYTES} bytes"),
            param: "body".to_string(),
        });
    }
    Ok(())
}

/// Validate snippet source size, syntax, and frontmatter/name consistency.
pub fn validate_snippet_body(name: &str, body: &str) -> Result<(), ToolError> {
    validate_snippet_body_structure(name, body)?;
    let code = if has_frontmatter(body) || body.contains("```") {
        extract_javascript_block(body)?
    } else {
        body.trim().to_string()
    };
    validate_snippet_code(&code)
}

/// Validate executable snippet JavaScript against the Code Mode source-size contract.
pub fn validate_snippet_code(code: &str) -> Result<(), ToolError> {
    let code = normalize_snippet_code(code);
    if code.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "snippet code is empty".to_string(),
            param: "body".to_string(),
        });
    }
    if !(code.starts_with("async ") && code.contains("=>")) {
        return Err(ToolError::InvalidParam {
            message:
                "snippet code must be an async arrow function, e.g. async () => ({ ok: true })"
                    .to_string(),
            param: "body".to_string(),
        });
    }

    // Parse the exact expression with the same QuickJS/Javy engine used by
    // Code Mode, but never evaluate it. `compile_to_bytecode` declares a
    // module and serializes bytecode only, so catalog discovery and explicit
    // validation cannot execute snippet side effects. This closes the gap
    // where a string could satisfy the cheap `async`/`=>` envelope check but
    // still fail only when a real Code Mode execution tried to parse it.
    let mut config = javy::Config::default();
    config.memory_limit(64 * 1024 * 1024);
    let runtime = javy::Runtime::new(config).map_err(|error| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("unable to initialize JavaScript validator: {error}"),
    })?;
    // Keep generated delimiters on their own lines. A valid snippet may end
    // in a // comment, which must not consume the validator's closing `);`.
    let source = format!("export default (\n{code}\n);");
    runtime
        .compile_to_bytecode("snippet-validation.js", &source)
        .map_err(|error| {
            let mut message = error.to_string();
            if message.len() > 1024 {
                let mut end = 1024;
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                message.truncate(end);
                message.push_str("...");
            }
            ToolError::InvalidParam {
                message: format!("snippet JavaScript is invalid: {message}"),
                param: "body".to_string(),
            }
        })?;
    Ok(())
}

/// Strip the opening `---` frontmatter delimiter line, tolerating both LF and
/// CRLF endings. Windows checkouts (and Windows-authored user snippets) carry
/// `---\r\n`, which a bare `strip_prefix("---\n")` would miss — silently
/// dropping the snippet's frontmatter and making built-ins undiscoverable.
fn strip_frontmatter_open(body: &str) -> Option<&str> {
    body.strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
}

/// Parse optional YAML snippet frontmatter from a Markdown snippet body.
pub fn frontmatter(body: &str) -> Result<Option<SnippetFrontmatter>, ToolError> {
    let Some(rest) = strip_frontmatter_open(body) else {
        return Ok(None);
    };
    let Some(raw) = frontmatter_block(rest) else {
        return Err(ToolError::InvalidParam {
            message: "snippet frontmatter starts with --- but is not closed".to_string(),
            param: "body".to_string(),
        });
    };
    let mut name = None;
    let mut description = None;
    let mut tags = Vec::new();
    let mut tools = None;
    let lines: Vec<&str> = raw.lines().collect();
    let mut inputs = BTreeMap::new();
    let mut i = 0;
    while i < lines.len() {
        let raw_line = lines[i];
        if raw_line.starts_with("  ") {
            i += 1;
            continue;
        }
        let line = raw_line.trim();
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "tools"
        {
            if tools.is_some() {
                return Err(tool_declarations::invalid(
                    "frontmatter tools must not be repeated",
                ));
            }
            let (parsed, next) = tool_declarations::parse(&lines, i + 1, value.trim())?;
            tools = Some(parsed);
            i = next;
            continue;
        }
        if line == "inputs:" {
            let (parsed, next) = parse_inputs_block(&lines, i + 1)?;
            inputs = parsed;
            i = next;
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(ToolError::InvalidParam {
                message: format!("invalid frontmatter line `{line}`"),
                param: "body".to_string(),
            });
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "name" => {
                name = Some(parse_scalar(
                    line.split_once(':').expect("delimiter parsed").1.trim(),
                ))
            }
            "description" => {
                description = Some(parse_scalar(
                    line.split_once(':').expect("delimiter parsed").1.trim(),
                ))
            }
            "tags" => tags = parse_tags(value)?,
            _ => {}
        }
        i += 1;
    }
    let name = required_frontmatter_field(name, "name")?;
    let description = required_frontmatter_field(description, "description")?;
    Ok(Some(SnippetFrontmatter {
        tools,
        name,
        description,
        tags,
        inputs,
    }))
}

fn frontmatter_block(rest: &str) -> Option<String> {
    let mut raw = Vec::new();
    for line in rest.lines() {
        if line.trim_end_matches('\r') == "---" {
            return Some(raw.join("\n"));
        }
        raw.push(line.trim_end_matches('\r'));
    }
    None
}

pub(super) fn has_frontmatter(body: &str) -> bool {
    strip_frontmatter_open(body).is_some()
}

fn required_frontmatter_field(value: Option<String>, field: &str) -> Result<String, ToolError> {
    let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
        return Err(ToolError::InvalidParam {
            message: format!("snippet frontmatter requires `{field}`"),
            param: "body".to_string(),
        });
    };
    Ok(value)
}

fn parse_tags(value: &str) -> Result<Vec<String>, ToolError> {
    let value = value.trim();
    if value.is_empty() || value == "[]" {
        return Ok(Vec::new());
    }
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(ToolError::InvalidParam {
            message: "frontmatter `tags` must be an inline array".to_string(),
            param: "body".to_string(),
        });
    };
    Ok(inner
        .split(',')
        .map(|tag| tag.trim().trim_matches('"').to_string())
        .filter(|tag| !tag.is_empty())
        .collect())
}

pub(super) fn render_user_snippet_body(
    name: &str,
    body: &str,
    description: Option<&str>,
) -> Result<String, ToolError> {
    if has_frontmatter(body) {
        return Ok(body.to_string());
    }
    let description = description
        .filter(|value| !value.trim().is_empty())
        .map(sanitize_frontmatter_scalar)
        .unwrap_or_else(|| "User snippet".to_string());
    let code = if body.contains("```") {
        extract_javascript_block(body)?
    } else {
        body.trim().to_string()
    };
    validate_snippet_code(&code)?;
    Ok(format!(
        "---\nname: {name}\ndescription: {description}\ntags: []\n---\n\n```js\n{code}\n```\n"
    ))
}

pub(super) fn sanitize_frontmatter_scalar(value: &str) -> String {
    let sanitized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "User snippet".to_string()
    } else {
        sanitized.replace('"', "'")
    }
}

fn parse_scalar(value: &str) -> String {
    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.trim_matches('"').to_string())
}
