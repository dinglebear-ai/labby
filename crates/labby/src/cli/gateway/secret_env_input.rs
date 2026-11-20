//! Standard-input framing for `labby server add|set --env NAME`.
//!
//! Each `--env NAME` takes one value from standard input, in flag order: one
//! value per line, LF-terminated (CRLF accepted, final newline optional).
//! Values never come from argv. Interactive input is refused, matching the
//! repository's other secret inputs (`--file -`, `--bearer-token-stdin`), so a
//! value is never echoed to a terminal. This module owns only framing and the
//! read bound; name and value policy belong to the shared gateway operation,
//! whose validators run here early so a bad name fails before stdin is read.

use std::collections::BTreeMap;
use std::io::Read;

use labby_gateway::security::stdio_secret_env::{
    MAX_SECRET_ENV_ENTRIES, MAX_SECRET_ENV_VALUE_BYTES, validate_secret_env_name,
    validate_secret_env_value,
};

use crate::dispatch::error::ToolError;

fn input_error(message: String) -> ToolError {
    ToolError::InvalidParam {
        message: format!("{message} No gateway action was dispatched."),
        param: "env".to_string(),
    }
}

/// Largest standard input accepted for `names`: every value at its cap plus a
/// CRLF terminator. One more byte proves the input is too large.
fn stdin_byte_limit(names: &[String]) -> usize {
    names.len() * (MAX_SECRET_ENV_VALUE_BYTES + 2)
}

/// Validate the `--env` names before any input is read.
pub(super) fn validate_secret_env_names(names: &[String]) -> Result<(), ToolError> {
    if names.len() > MAX_SECRET_ENV_ENTRIES {
        return Err(input_error(format!(
            "At most {MAX_SECRET_ENV_ENTRIES} --env flags may be given."
        )));
    }
    for (index, name) in names.iter().enumerate() {
        validate_secret_env_name(name)
            .map_err(|error| input_error(format!("{}.", error.user_message())))?;
        if names[..index].contains(name) {
            return Err(input_error(format!(
                "--env {name} is given more than once."
            )));
        }
    }
    Ok(())
}

/// Split `input` into one value per name, in order. Errors cite the name and
/// line number, never a value.
pub(super) fn parse_secret_env_values(
    names: &[String],
    input: &[u8],
) -> Result<BTreeMap<String, String>, ToolError> {
    if input.len() > stdin_byte_limit(names) {
        return Err(input_error(format!(
            "Standard input for --env exceeds {} bytes.",
            stdin_byte_limit(names)
        )));
    }
    let text = std::str::from_utf8(input)
        .map_err(|_| input_error("Standard input for --env is not valid UTF-8.".to_string()))?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let lines = if body.is_empty() && !text.is_empty() {
        vec![""]
    } else if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').collect::<Vec<_>>()
    };
    if lines.len() != names.len() {
        return Err(input_error(format!(
            "--env expects {} value line(s) on standard input, one per flag in order; got {}.",
            names.len(),
            lines.len()
        )));
    }
    let mut values = BTreeMap::new();
    for (index, (name, line)) in names.iter().zip(lines).enumerate() {
        let value = line.strip_suffix('\r').unwrap_or(line);
        validate_secret_env_value(name, value).map_err(|error| {
            input_error(format!(
                "{} (standard input line {}).",
                error.user_message(),
                index + 1
            ))
        })?;
        values.insert(name.clone(), value.to_string());
    }
    Ok(values)
}

/// Read and frame the values for `names` from standard input.
pub(super) fn read_secret_env_values(
    names: &[String],
) -> Result<BTreeMap<String, String>, ToolError> {
    use std::io::IsTerminal as _;

    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    validate_secret_env_names(names)?;
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(input_error(
            "--env reads values from standard input and refuses a terminal so a value is never echoed. Pipe one value per line, in flag order, for example `printf '%s\\n' \"$TOKEN\" | labby server add NAME --command npx --env NAME`.".to_string(),
        ));
    }
    let mut input = Vec::new();
    stdin
        .lock()
        .take(stdin_byte_limit(names) as u64 + 1)
        .read_to_end(&mut input)
        .map_err(|error| {
            input_error(format!(
                "Cannot read --env values from standard input: {}.",
                error.kind()
            ))
        })?;
    parse_secret_env_values(names, &input)
}

/// Attach write-only `--env` values. They travel only in the dispatch body.
pub(super) fn insert_secret_env_values(
    params: &mut serde_json::Value,
    values: BTreeMap<String, String>,
) {
    if !values.is_empty()
        && let Some(params) = params.as_object_mut()
    {
        params.insert("secret_env_values".to_string(), serde_json::json!(values));
    }
}

/// Replace every `--env` value with a marker before a preview is rendered.
pub(super) fn redact_secret_env_values(mut params: serde_json::Value) -> serde_json::Value {
    if let Some(values) = params
        .get_mut("secret_env_values")
        .and_then(serde_json::Value::as_object_mut)
    {
        for value in values.values_mut() {
            *value = serde_json::json!("[redacted]");
        }
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_A: &str = "fake-cli-value-a-19c4";
    const FAKE_B: &str = "fake-cli-value-b-6e02";

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn rendered(error: &ToolError) -> String {
        format!("{error} {}", serde_json::to_string(error).unwrap())
    }

    #[test]
    fn values_follow_flag_order() {
        let names = names(&["OTHER_TOKEN", "FIGMA_API_KEY"]);
        let input = format!("{FAKE_A}\n{FAKE_B}\n");

        let values = parse_secret_env_values(&names, input.as_bytes()).expect("values");

        assert_eq!(values["OTHER_TOKEN"], FAKE_A);
        assert_eq!(values["FIGMA_API_KEY"], FAKE_B);
    }

    #[test]
    fn accepts_crlf_and_a_missing_final_newline() {
        let names = names(&["A_KEY", "B_KEY"]);
        let values = parse_secret_env_values(&names, format!("{FAKE_A}\r\n{FAKE_B}").as_bytes())
            .expect("values");
        assert_eq!(values["A_KEY"], FAKE_A);
        assert_eq!(values["B_KEY"], FAKE_B);
    }

    #[test]
    fn keeps_inner_spaces_and_shell_characters_verbatim() {
        let names = names(&["A_KEY"]);
        let value = "fake value with $HOME \"quotes\" #hash";
        let values =
            parse_secret_env_values(&names, format!("{value}\n").as_bytes()).expect("values");
        assert_eq!(values["A_KEY"], value);
    }

    #[test]
    fn refuses_line_count_mismatch_without_echoing_values() {
        let cases = [
            (names(&["A_KEY", "B_KEY"]), format!("{FAKE_A}\n")),
            (names(&["A_KEY"]), format!("{FAKE_A}\n{FAKE_B}\n")),
            (names(&["A_KEY"]), String::new()),
        ];
        for (names, input) in cases {
            let error = parse_secret_env_values(&names, input.as_bytes()).unwrap_err();
            let text = rendered(&error);
            assert!(text.contains("value line(s)"), "{text}");
            assert!(!text.contains(FAKE_A) && !text.contains(FAKE_B));
        }
    }

    #[test]
    fn refuses_empty_values() {
        for input in ["\n", "   \n", "\r\n"] {
            let error = parse_secret_env_values(&names(&["A_KEY"]), input.as_bytes()).unwrap_err();
            assert!(rendered(&error).contains("must not be empty"));
        }
        let error = parse_secret_env_values(
            &names(&["A_KEY", "B_KEY"]),
            format!("\n{FAKE_B}\n").as_bytes(),
        )
        .unwrap_err();
        let text = rendered(&error);
        assert!(text.contains("A_KEY") && text.contains("line 1"), "{text}");
        assert!(!text.contains(FAKE_B));
    }

    #[test]
    fn caps_each_value_and_the_whole_input() {
        let names_one = names(&["A_KEY"]);
        let at_cap = "a".repeat(MAX_SECRET_ENV_VALUE_BYTES);
        parse_secret_env_values(&names_one, format!("{at_cap}\r\n").as_bytes())
            .expect("value at cap");

        let over_value = format!("{FAKE_A}{}\n", "a".repeat(MAX_SECRET_ENV_VALUE_BYTES));
        let error = parse_secret_env_values(&names_one, over_value.as_bytes()).unwrap_err();
        assert!(!rendered(&error).contains(FAKE_A));

        let over_total = "a".repeat(stdin_byte_limit(&names_one) + 1);
        let error = parse_secret_env_values(&names_one, over_total.as_bytes()).unwrap_err();
        assert!(rendered(&error).contains("exceeds"));
    }

    #[test]
    fn refuses_invalid_reserved_and_duplicate_names() {
        for list in [
            names(&["figma_api_key"]),
            names(&["PATH"]),
            names(&["LABBY_TOKEN"]),
            names(&["LD_PRELOAD"]),
            names(&["TMPDIR"]),
            names(&["A_KEY", "A_KEY"]),
        ] {
            assert!(validate_secret_env_names(&list).is_err(), "{list:?}");
        }
        let too_many = (0..=MAX_SECRET_ENV_ENTRIES)
            .map(|index| format!("KEY_{index}"))
            .collect::<Vec<_>>();
        assert!(validate_secret_env_names(&too_many).is_err());
        validate_secret_env_names(&names(&["FIGMA_API_KEY", "OTHER_TOKEN"])).expect("valid");
    }

    fn parsed(args: &[&str]) -> crate::cli::gateway::GatewayCommand {
        use clap::Parser as _;
        let cli = crate::cli::Cli::try_parse_from(args).expect("parse");
        let crate::cli::Command::Gateway(gateway) = cli.command.into_operation() else {
            panic!("expected gateway command");
        };
        gateway.command
    }

    #[test]
    fn server_add_and_set_take_repeatable_env_names_only() {
        let crate::cli::gateway::GatewayCommand::Add(add) = parsed(&[
            "labby",
            "server",
            "add",
            "figma",
            "--command",
            "npx",
            "--arg=-y",
            "--arg",
            "figma-developer-mcp",
            "--arg=--stdio",
            "--env",
            "FIGMA_API_KEY",
            "--env",
            "OTHER_TOKEN",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(add.env, names(&["FIGMA_API_KEY", "OTHER_TOKEN"]));
        assert_eq!(add.args, names(&["-y", "figma-developer-mcp", "--stdio"]));

        let crate::cli::gateway::GatewayCommand::Update(set) =
            parsed(&["labby", "server", "set", "figma", "--env", "FIGMA_API_KEY"])
        else {
            panic!("expected set");
        };
        assert_eq!(set.env, names(&["FIGMA_API_KEY"]));
        assert_eq!(set.args, Vec::<String>::new());
    }

    #[test]
    fn dispatch_body_carries_values_and_previews_redact_them() {
        let mut params = serde_json::json!({ "name": "figma" });
        insert_secret_env_values(&mut params, BTreeMap::new());
        assert!(params.get("secret_env_values").is_none());

        let values = parse_secret_env_values(
            &names(&["FIGMA_API_KEY", "OTHER_TOKEN"]),
            format!("{FAKE_A}\n{FAKE_B}\n").as_bytes(),
        )
        .expect("values");
        insert_secret_env_values(&mut params, values);
        assert_eq!(params["secret_env_values"]["FIGMA_API_KEY"], FAKE_A);

        let preview = crate::cli::helpers::dry_run_value(
            "gateway",
            "gateway.add",
            &redact_secret_env_values(params),
        )
        .to_string();
        assert!(preview.contains("secret_env_values") && preview.contains("[redacted]"));
        assert!(!preview.contains(FAKE_A) && !preview.contains(FAKE_B));
    }

    #[test]
    fn refuses_invalid_utf8() {
        let error = parse_secret_env_values(&names(&["A_KEY"]), &[0xff, b'\n']).unwrap_err();
        assert!(rendered(&error).contains("UTF-8"));
    }
}
