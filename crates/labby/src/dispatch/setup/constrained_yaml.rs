use serde::de::DeserializeOwned;
use serde_yaml_ng::Value;

const MAX_BYTES: usize = 1_048_576;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 10_000;
const MAX_COLLECTION_ITEMS: usize = 2_048;

pub(super) fn parse<T: DeserializeOwned>(yaml: &str) -> Result<T, String> {
    if yaml.len() > MAX_BYTES {
        return Err(format!("YAML exceeds {MAX_BYTES} bytes"));
    }
    reject_graph_syntax(yaml)?;
    let value: Value = serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())?;
    let mut nodes = 0;
    validate_value(&value, 0, &mut nodes)?;
    serde_yaml_ng::from_value(value).map_err(|error| error.to_string())
}

fn reject_graph_syntax(yaml: &str) -> Result<(), String> {
    let mut block_indent: Option<usize> = None;
    for line in yaml.lines() {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if block_indent.is_some_and(|parent| line.trim().is_empty() || indent > parent) {
            continue;
        }
        block_indent = None;
        let code = unquoted_code(line);
        if code.trim_end().ends_with('|')
            || code.trim_end().ends_with("|-")
            || code.trim_end().ends_with('|')
            || code.trim_end().ends_with('>')
            || code.trim_end().ends_with(">-")
        {
            block_indent = Some(indent);
        }
        for (index, character) in code.char_indices() {
            if !matches!(character, '&' | '*' | '!') {
                continue;
            }
            let before = code[..index].chars().next_back();
            let after = code[index + character.len_utf8()..].chars().next();
            if before.is_none_or(|value| value.is_whitespace() || "[{,:-?".contains(value))
                && after.is_some_and(|value| !value.is_whitespace() && !"}],".contains(value))
            {
                return Err("YAML anchors, aliases, and tags are not allowed".into());
            }
        }
    }
    Ok(())
}

fn unquoted_code(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            output.push(' ');
            escaped = false;
            continue;
        }
        if double && character == '\\' {
            output.push(' ');
            escaped = true;
        } else if !double && character == '\'' {
            single = !single;
            output.push(' ');
        } else if !single && character == '"' {
            double = !double;
            output.push(' ');
        } else if !single && !double && character == '#' {
            break;
        } else if single || double {
            output.push(' ');
        } else {
            output.push(character);
        }
    }
    output
}

fn validate_value(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("YAML nesting exceeds {MAX_DEPTH} levels"));
    }
    *nodes += 1;
    if *nodes > MAX_NODES {
        return Err(format!("YAML exceeds {MAX_NODES} nodes"));
    }
    match value {
        Value::Sequence(values) => {
            if values.len() > MAX_COLLECTION_ITEMS {
                return Err(format!(
                    "YAML collection exceeds {MAX_COLLECTION_ITEMS} items"
                ));
            }
            for value in values {
                validate_value(value, depth + 1, nodes)?;
            }
        }
        Value::Mapping(values) => {
            if values.len() > MAX_COLLECTION_ITEMS {
                return Err(format!(
                    "YAML collection exceeds {MAX_COLLECTION_ITEMS} items"
                ));
            }
            for (key, value) in values {
                if !matches!(key, Value::String(_)) {
                    return Err("YAML mapping keys must be explicit strings".into());
                }
                validate_value(value, depth + 1, nodes)?;
            }
        }
        Value::Tagged(_) => return Err("YAML tags are not allowed".into()),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;
    use serde_yaml_ng::Value;

    #[test]
    fn rejects_graph_features_in_mapping_and_sequence_positions() {
        for yaml in [
            "value: &anchor text\nother: *anchor\n",
            "values:\n  - &anchor text\n  - *anchor\n",
            "values:\n  - !unsafe text\n",
        ] {
            assert!(parse::<Value>(yaml).is_err(), "accepted {yaml:?}");
        }
    }

    #[test]
    fn ignores_graph_tokens_inside_quotes_comments_and_block_scalars() {
        let yaml = "quoted: 'safe & literal' # * comment\nscript: |-\n  echo '! command'\n";
        assert!(parse::<Value>(yaml).is_ok());
    }

    #[test]
    fn rejects_duplicate_keys_and_bounded_resource_exhaustion() {
        assert!(parse::<Value>("value: one\nvalue: two\n").is_err());
        assert!(parse::<Value>(&format!("value: {}\n", "x".repeat(1_048_577))).is_err());
        assert!(parse::<Value>(&format!("{} value\n", "- ".repeat(40))).is_err());
        let broad = format!("values:\n{}", "  - value\n".repeat(2_049));
        assert!(parse::<Value>(&broad).is_err());
    }
}
