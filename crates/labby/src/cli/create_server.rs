//! Guided creation fills the existing typed upstream arguments, not a second API.

use crate::{cli::gateway::GatewayAddArgs, config::cli::invalid};
use anyhow::Result;
use dialoguer::{Confirm, Input, Select};
use std::io::Write as _;

trait CreationPrompt {
    fn text(&mut self, question: &str) -> Result<String>;
    fn use_http(&mut self) -> Result<bool>;
    fn confirm(&mut self, invocation: &str) -> Result<bool>;
}

struct TerminalPrompt {
    selectors: String,
}
impl CreationPrompt for TerminalPrompt {
    fn text(&mut self, question: &str) -> Result<String> {
        Ok(Input::<String>::new()
            .with_prompt(question)
            .interact_text()?)
    }
    fn use_http(&mut self) -> Result<bool> {
        Ok(Select::new()
            .with_prompt("Upstream transport")
            .items(["HTTP MCP server", "Local stdio program"])
            .default(0)
            .interact()?
            == 0)
    }
    fn confirm(&mut self, invocation: &str) -> Result<bool> {
        let invocation = invocation.replacen("labby ", &format!("labby {}", self.selectors), 1);
        writeln!(
            std::io::stderr().lock(),
            "Equivalent command (sensitive values are redacted):\n  {invocation}"
        )?;
        Ok(Confirm::new()
            .with_prompt("Add this upstream server?")
            .default(false)
            .interact()?)
    }
}

fn quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._/:=".contains(&byte))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn invocation(args: &GatewayAddArgs) -> String {
    let safe = crate::cli::helpers::diagnostic_value(
        &serde_json::json!({
            "name":args.name, "url":args.url, "command":args.command, "args":args.args,
            "bearer_token_env":args.bearer_token_env, "expose_skills":args.expose_skills,
        }),
        8192,
    );
    let mut words = vec![
        "labby".to_owned(),
        "server".to_owned(),
        "add".to_owned(),
        safe["name"].as_str().unwrap_or("[redacted]").to_owned(),
    ];
    for key in ["url", "command", "bearer_token_env"] {
        if let Some(value) = safe[key].as_str() {
            words.extend([format!("--{}", key.replace('_', "-")), value.to_owned()]);
        }
    }
    for (key, flag) in [("args", "--arg"), ("expose_skills", "--expose-skill")] {
        if let Some(values) = safe[key].as_array() {
            for value in values.iter().filter_map(serde_json::Value::as_str) {
                // Bind flag-like values before shell quoting; `--arg -y`
                // would be parsed as a new CLI option when replayed.
                words.push(format!("{flag}={value}"));
            }
        }
    }
    words.extend([
        "--proxy-resources".to_owned(),
        args.proxy_resources.to_string(),
        "--proxy-skills".to_owned(),
        args.proxy_skills.to_string(),
    ]);
    if args.dry_run {
        words.push("--dry-run".to_owned());
    }
    words
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn complete_with(
    args: &mut GatewayAddArgs,
    interactive: bool,
    prompt: &mut impl CreationPrompt,
) -> Result<()> {
    let needs_input = args.name.is_empty() || args.url.is_none() && args.command.is_none();
    if !needs_input {
        return Ok(());
    }
    if !interactive {
        return Err(invalid("Missing upstream name or transport. Use labby server add NAME --url URL, or --command PROGRAM. Guided input requires an interactive terminal without --json or --no-input. No operation was executed.").into());
    }
    if args.name.is_empty() {
        args.name = prompt.text("Upstream name")?.trim().to_owned();
    }
    if args.url.is_none() && args.command.is_none() {
        if prompt.use_http()? {
            args.url = Some(prompt.text("MCP URL")?.trim().to_owned());
        } else {
            args.command = Some(
                prompt
                    .text("Absolute program path or command name")?
                    .trim()
                    .to_owned(),
            );
        }
    }
    if args.name.is_empty()
        || args
            .url
            .as_ref()
            .or(args.command.as_ref())
            .is_none_or(|value| value.is_empty())
    {
        return Err(invalid("Name and transport must not be empty. No upstream was added.").into());
    }
    if !prompt.confirm(&invocation(args))? {
        return Err(crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "confirmation_required".into(),
            message: "Upstream creation was cancelled. No operation was executed.".into(),
        }
        .into());
    }
    Ok(())
}

pub fn prepare(
    args: &mut GatewayAddArgs,
    interactive: bool,
    target: Option<&crate::config::cli::SelectedTarget>,
    team_id: Option<&str>,
) -> Result<()> {
    let mut selectors = String::new();
    if let Some(target) = target {
        selectors.push_str(&format!("--server {} ", quote(&target.server)));
    }
    if let Some(team_id) = team_id {
        selectors.push_str(&format!("--team-id {} ", quote(team_id)));
    }
    complete_with(args, interactive, &mut TerminalPrompt { selectors })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Prompt {
        texts: std::collections::VecDeque<String>,
        calls: usize,
        approved: bool,
        command: String,
    }
    impl CreationPrompt for Prompt {
        fn text(&mut self, _: &str) -> Result<String> {
            self.calls += 1;
            Ok(self.texts.pop_front().expect("expected prompt"))
        }
        fn use_http(&mut self) -> Result<bool> {
            self.calls += 1;
            Ok(true)
        }
        fn confirm(&mut self, command: &str) -> Result<bool> {
            self.calls += 1;
            self.command = command.to_owned();
            Ok(self.approved)
        }
    }
    fn args() -> GatewayAddArgs {
        GatewayAddArgs {
            name: String::new(),
            url: None,
            command: None,
            args: vec![],
            bearer_token_env: None,
            proxy_resources: true,
            proxy_skills: false,
            expose_skills: vec![],
            dry_run: false,
        }
    }
    #[test]
    fn guided_creation_uses_the_same_typed_arguments_and_prints_a_reproducible_command() {
        let mut input = args();
        let mut prompt = Prompt {
            texts: ["docs", "https://example.com/mcp"]
                .map(str::to_owned)
                .into(),
            approved: true,
            ..Default::default()
        };
        complete_with(&mut input, true, &mut prompt).unwrap();
        assert_eq!(input.name, "docs");
        assert_eq!(input.url.as_deref(), Some("https://example.com/mcp"));
        assert_eq!(prompt.calls, 4);
        assert!(
            prompt
                .command
                .contains("labby server add docs --url https://example.com/mcp")
        );
    }
    #[test]
    fn noninteractive_and_complete_calls_never_prompt() {
        let mut input = args();
        let mut prompt = Prompt::default();
        assert!(complete_with(&mut input, false, &mut prompt).is_err());
        assert_eq!(prompt.calls, 0);
        input.name = "docs".into();
        input.url = Some("https://example.com/mcp".into());
        complete_with(&mut input, true, &mut prompt).unwrap();
        assert_eq!(prompt.calls, 0);
    }
    #[test]
    fn declining_confirmation_cannot_reach_dispatch() {
        let mut input = args();
        let mut prompt = Prompt {
            texts: ["docs", "https://example.com/mcp"]
                .map(str::to_owned)
                .into(),
            ..Default::default()
        };
        let error = complete_with(&mut input, true, &mut prompt).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<crate::dispatch::error::ToolError>()
                .unwrap()
                .kind(),
            "confirmation_required"
        );
        assert_eq!(prompt.calls, 4);
    }
    #[test]
    fn equivalent_command_quotes_shell_metacharacters() {
        assert_eq!(quote("abc"), "abc");
        assert_eq!(quote("$(id)"), "'$(id)'");
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn equivalent_command_preserves_flag_like_upstream_arguments() {
        use clap::Parser as _;
        let mut input = args();
        input.name = "docs".into();
        input.command = Some("npx".into());
        input.args = vec!["-y".into(), "--version".into()];
        let command = invocation(&input);
        let parsed = crate::cli::Cli::try_parse_from(command.split_whitespace()).unwrap();
        let crate::cli::Command::Server(crate::cli::server::ServerArgs {
            command: crate::cli::server::ServerCommand::Add(actual),
        }) = parsed.command
        else {
            panic!("expected server add");
        };
        assert_eq!(actual.args, input.args);
        assert_eq!(actual.command, input.command);
    }
}
