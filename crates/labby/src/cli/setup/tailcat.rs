//! Thin native Tailcat onboarding adapter.
use std::process::ExitCode;

use anyhow::{Result, bail};

use super::{SetupArgs, SetupAuthArg, SetupOauthArg, SetupRoleArg};
use crate::output::{OutputFormat, print};

pub(super) fn run(
    mut args: SetupArgs,
    interactive: bool,
    format: OutputFormat,
) -> Result<ExitCode> {
    if args.skip_deps {
        bail!("Tailcat setup uses verified release companions; --skip-deps is not supported");
    }
    if matches!(args.auth, Some(SetupAuthArg::Bearer | SetupAuthArg::Both))
        || args.oauth == Some(SetupOauthArg::None)
    {
        bail!("Tailcat dashboard setup requires OAuth-only authentication");
    }
    let paths = crate::installation::InstallationPaths::resolve()?;
    let mut outcome = crate::dispatch::setup::tailcat::inspect(&paths.config_toml())?;
    args.role = Some(SetupRoleArg::Server);
    if args.auth.is_some()
        || args.oauth.is_some()
        || !crate::dispatch::setup::tailcat::has_existing_oauth(paths.root())?
    {
        args.auth = Some(SetupAuthArg::OAuth);
    }
    args.config_only = true;
    // Reuse persistent signing/encryption material and the existing provider setup.
    outcome["authentication"] = super::onboarding::configure_only(&args, interactive)?;
    outcome["dry_run"] = serde_json::json!(args.dry_run);
    if !args.dry_run {
        crate::dispatch::setup::tailcat::prepare(&paths.config_toml())?;
    }
    print(&outcome, format)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    #[test]
    fn native_mode_rejects_proxy_and_daemon_modes() {
        for conflict in ["--chatgpt", "--provision", "--desktop"] {
            assert!(
                crate::cli::Cli::try_parse_from(["labby", "setup", "--tailcat", conflict]).is_err()
            );
        }
        assert!(
            crate::cli::Cli::try_parse_from(["labby", "setup", "--tailcat", "--dry-run"]).is_ok()
        );
    }
}
