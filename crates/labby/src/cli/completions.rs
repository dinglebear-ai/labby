//! Shell completion is generated from Clap; optional resource names are cached locally.
use crate::{
    cli::Cli,
    config::LabConfig,
    output::{OutputFormat, print},
};
use anyhow::Result;
use clap::{Args, CommandFactory, Subcommand};
use clap_complete::{Shell, generate};
use std::{
    io::{self, Write as _},
    process::ExitCode,
};

#[derive(Debug, Args)]
#[command(arg_required_else_help = true, args_conflicts_with_subcommands = true)]
pub struct CompletionsArgs {
    /// Generate a script for this shell.
    #[arg(value_enum)]
    pub shell: Option<Shell>,
    /// Include offline, authority-scoped cached resource-name suggestions.
    #[arg(long, requires = "shell")]
    pub resources: bool,
    #[command(subcommand)]
    pub command: Option<CompletionCommand>,
}

#[derive(Debug, Subcommand)]
pub enum CompletionCommand {
    /// Suggest from the command graph and a valid local cache; never contacts a gateway.
    Query {
        /// Words after labby, ending with the current partial word (which may be empty).
        #[arg(last = true)]
        words: Vec<String>,
    },
    /// Explicitly refresh resource names for the selected gateway and authority.
    #[cfg(feature = "gateway")]
    Refresh,
    /// Remove only the selected authority's local resource-name cache.
    #[cfg(feature = "gateway")]
    Clear,
}

impl CompletionsArgs {
    pub fn metadata_only(&self) -> bool {
        matches!(self.command, None | Some(CompletionCommand::Query { .. }))
    }
}

fn script(shell: Shell, resources: bool) -> Result<String> {
    let mut bytes = Vec::new();
    generate(shell, &mut Cli::command(), "labby", &mut bytes);
    let mut script = String::from_utf8(bytes)?;
    if !resources {
        return Ok(script);
    }
    let extension = match shell {
        Shell::Bash => {
            r#"
# Cached suggestions only: no daemon discovery or OAuth during completion.
_labby_with_resources() {
    _labby "$@"
    local candidate
    while IFS= read -r candidate; do
        COMPREPLY+=("$candidate")
    done < <(command labby completions query -- "${COMP_WORDS[@]:1:COMP_CWORD}" 2>/dev/null)
}
complete -o bashdefault -o default -F _labby_with_resources labby
"#
        }
        Shell::Zsh => {
            r#"
_labby_with_resources() {
    _labby "$@"
    local -a cached
    cached=("${(@f)$(command labby completions query -- "${words[@]:1:$((CURRENT-1))}" 2>/dev/null)}")
    (( ${#cached} )) && compadd -- "${cached[@]}"
    return 0
}
compdef _labby_with_resources labby
"#
        }
        Shell::Fish => {
            r#"
function __labby_cached_resources
    command labby completions query -- (commandline -opc)[2..-1] (commandline -ct) 2>/dev/null
end
complete -c labby -a "(__labby_cached_resources)"
"#
        }
        Shell::PowerShell => {
            let marker = "Register-ArgumentCompleter -Native -CommandName 'labby' -ScriptBlock ";
            anyhow::ensure!(
                script.matches(marker).count() == 1,
                "PowerShell completion generator changed its registration contract"
            );
            script = script.replacen(marker, "$script:LabbyStaticCompletion = ", 1);
            r#"
Register-ArgumentCompleter -Native -CommandName 'labby' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    & $script:LabbyStaticCompletion $wordToComplete $commandAst $cursorPosition
    $literalWords = [System.Collections.Generic.List[string]]::new()
    foreach ($element in $commandAst.CommandElements | Select-Object -Skip 1) {
        if ($element.Extent.EndOffset -ge $cursorPosition) { break }
        if ($element -is [StringConstantExpressionAst]) {
            $literalWords.Add($element.Value)
        } elseif ($element -is [CommandParameterAst] -and $null -eq $element.Argument) {
            $literalWords.Add($element.Extent.Text)
        } else { return }
    }
    $literalWords.Add([string]$wordToComplete)
    $queryWords = $literalWords.ToArray()
    & labby completions query -- @queryWords 2>$null | ForEach-Object {
        [CompletionResult]::new($_, $_, [CompletionResultType]::ParameterValue, $_)
    }
}
"#
        }
        Shell::Elvish => {
            r#"
var labby-static-completer = $edit:completion:arg-completer[labby]
set edit:completion:arg-completer[labby] = {|@words|
    try { $labby-static-completer $@words } catch { }
    var query-words = $words[1..]
    try { labby completions query -- $@query-words 2>/dev/null } catch { }
}
"#
        }
        _ => anyhow::bail!("Cached resource completion is not supported for this shell"),
    };
    script.push_str(extension);
    Ok(script)
}

pub async fn run(
    args: CompletionsArgs,
    config: &LabConfig,
    server: Option<String>,
    context: Option<String>,
    team: Option<String>,
    format: OutputFormat,
) -> Result<ExitCode> {
    match args.command {
        None => {
            let shell = args.shell.ok_or_else(|| {
                crate::config::cli::invalid(
                    "Specify a shell or use completions query, refresh, or clear.",
                )
            })?;
            let script = script(shell, args.resources)?;
            if format.is_json() {
                print(
                    &serde_json::json!({"shell":shell.to_string(),"resources":args.resources,"script":script}),
                    format,
                )?;
            } else {
                io::stdout().lock().write_all(script.as_bytes())?;
            }
        }
        Some(CompletionCommand::Query { words }) => {
            let mut selectors = Vec::new();
            for (flag, value) in [
                ("--server", server),
                ("--context", context),
                ("--team-id", team),
            ] {
                if let Some(value) = value {
                    selectors.extend([flag.to_owned(), value]);
                }
            }
            selectors.extend(words);
            let values = super::completion_cache::query(&selectors);
            if format.is_json() {
                print(&values, format)?;
            } else {
                let mut stdout = io::stdout().lock();
                for value in values {
                    writeln!(stdout, "{value}")?;
                }
            }
        }
        #[cfg(feature = "gateway")]
        Some(CompletionCommand::Refresh) => {
            let (value, ok) = super::completion_cache::refresh(config, team.as_deref()).await?;
            print(&value, format)?;
            return Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        #[cfg(feature = "gateway")]
        Some(CompletionCommand::Clear) => print(
            &super::completion_cache::clear(config, team.as_deref())?,
            format,
        )?,
    }
    #[cfg(not(feature = "gateway"))]
    let _ = config;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_shells_keep_static_discovery_and_use_only_offline_queries() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let static_script = script(shell, false).unwrap();
            let cached = script(shell, true).unwrap();
            assert!(static_script.contains("context"));
            assert!(cached.contains("completions query --"));
            assert!(
                !cached.contains("labby completions refresh"),
                "Tab must not refresh remotely"
            );
            assert!(cached.len() > static_script.len());
        }
    }
}
