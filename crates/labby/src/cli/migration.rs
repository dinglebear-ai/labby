//! Retirement guidance only. Old spellings are never parsed or executed as aliases.

use clap::CommandFactory;
use std::ffi::OsString;

/// Command-prefix moves, not a second parser or action catalog.
pub const MOVED: &[(&str, &str)] = &[
    (
        "gateway mcp auth revoke-google",
        "auth provider google revoke",
    ),
    ("gateway mcp auth start", "server auth login --no-browser"),
    ("gateway mcp auth open", "server auth login"),
    ("gateway mcp auth clear", "server auth logout"),
    ("gateway mcp auth status", "server auth status"),
    ("gateway mcp auth", "server auth"),
    ("gateway mcp list", "server status"),
    ("gateway mcp enable", "server enable"),
    ("gateway mcp disable", "server disable"),
    ("gateway mcp restart", "server restart"),
    ("gateway mcp cleanup", "server cleanup"),
    ("gateway mcp", "server"),
    ("gateway protected-route update", "route replace"),
    ("gateway protected-route", "route"),
    ("gateway loadout update", "loadout set"),
    ("gateway loadout", "loadout"),
    ("gateway code exec", "code run"),
    ("gateway code", "code"),
    ("gateway enrich apply", "code hints apply"),
    ("gateway enrich", "code hints preview"),
    ("gateway skills expose-all", "skill source exposure clear"),
    ("gateway skills expose", "skill source exposure set"),
    ("gateway skills", "skill source"),
    ("gateway public-urls", "gateway urls"),
    ("gateway clients", "gateway sessions"),
    ("gateway update", "server set"),
    ("gateway list", "server list"),
    ("gateway get", "server get"),
    ("gateway test", "server test"),
    ("gateway add", "server add"),
    ("gateway remove", "server remove"),
    ("gateway pending", "server pending"),
    ("gateway quarantine", "server quarantine"),
    ("gateway discover", "server discover"),
    ("gateway import", "server import"),
    ("setup host-service", "host service"),
    ("setup incusbackup", "host incus backup"),
    ("setup incus-backup", "host incus backup"),
    ("setup incus-ssh", "host incus ssh"),
    ("setup access-bootstrap", "auth bootstrap"),
    ("setup owner-link-prepare", "auth owner link"),
    ("setup draft", "config draft"),
    ("setup proxy", "config proxy set"),
    ("setup install", "host install"),
    ("oauth relay-local", "auth relay local"),
    ("oauth relay-registry", "auth relay registry"),
    ("oauth", "auth relay"),
    ("state migrate-access", "state access migrate"),
    ("doctor oauth-relay", "doctor relay"),
    ("snippets exec", "snippet run"),
    ("snippets create", "snippet add"),
    ("snippets", "snippet"),
    ("skills", "skill"),
    ("incus", "host incus"),
    ("login", "auth login"),
    ("update", "host update"),
    ("health", "gateway status"),
];

/// Explain a retired prefix without echoing operands, credentials, or arbitrary values.
#[must_use]
pub fn hint(argv: &[OsString]) -> Option<String> {
    let mut root = super::Cli::command();
    root.build();
    let mut arguments = argv.iter().skip(1).peekable();
    while let Some(token) = arguments.peek() {
        let token = token.to_str()?;
        if token == "--" {
            return None;
        }
        if let Some(flag) = token.strip_prefix("--") {
            let (name, inline) = flag
                .split_once('=')
                .map_or((flag, false), |(name, _)| (name, true));
            let argument = root
                .get_arguments()
                .find(|arg| arg.get_long() == Some(name))?;
            arguments.next();
            if argument.get_action().takes_values() && !inline {
                arguments.next()?;
            }
        } else if token.starts_with('-') {
            if !token
                .chars()
                .skip(1)
                .all(|character| matches!(character, 'v' | 'q' | 'h' | 'V'))
            {
                return None;
            }
            arguments.next();
        } else {
            break;
        }
    }
    let command = arguments
        .map(|arg| arg.to_string_lossy())
        .take_while(|arg| !arg.starts_with('-'))
        .collect::<Vec<_>>()
        .join(" ");
    let (old, new) = MOVED
        .iter()
        .filter(|(old, _)| command == *old || command.starts_with(&format!("{old} ")))
        .max_by_key(|(old, _)| old.len())?;
    if !root
        .get_subcommands()
        .any(|child| Some(child.get_name()) == new.split_whitespace().next())
    {
        return None;
    }
    Some(format!(
        "`labby {old}` has been retired, not aliased. Use `labby {new}`. Run `labby {new} --help` for its operands and options. Server and route names are positional; route replace still replaces the full configuration."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_uses_command_positions_and_never_echoes_operands() {
        let args = [
            "labby",
            "--team-id",
            "private-team",
            "setup",
            "host-service",
            "restart",
            "arbitrary-secret",
        ]
        .map(OsString::from);
        let message = hint(&args).unwrap();
        assert!(message.contains("labby host service"));
        assert!(!message.contains("private-team"));
        assert!(!message.contains("arbitrary-secret"));
        assert!(hint(&["labby", "--team-id", "login", "doctor"].map(OsString::from)).is_none());
        assert!(hint(&["labby", "--", "login"].map(OsString::from)).is_none());
    }

    #[test]
    #[cfg(feature = "gateway")]
    fn every_replacement_is_a_real_help_path() {
        use clap::Parser;
        for (_, replacement) in MOVED {
            let args = std::iter::once("labby")
                .chain(replacement.split_whitespace())
                .chain(std::iter::once("--help"));
            let error = super::super::Cli::try_parse_from(args).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp,
                "invalid replacement: {replacement}: {error}"
            );
        }
    }
}
