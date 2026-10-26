//! `labby host update` — install the latest release, then refresh Incus when present.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::output::{OutputFormat, print};

#[derive(Debug, Args, Clone)]
pub struct UpdateArgs {
    /// Release channel; stable by default. Automatic checks retain the saved selection.
    #[arg(long, value_enum)]
    pub channel: Option<crate::self_update::UpdateChannel>,
    /// Install a newer selected-channel release on this host only (macOS Apple Silicon).
    #[arg(long, conflicts_with_all = ["auto_update", "version", "install_dir", "container", "check_url", "force_fallback", "no_force_fallback", "no_web_assets"])]
    pub automatic: bool,
    /// Enable, disable, or inspect daily native macOS updates.
    #[arg(long, value_parser = ["enable", "disable", "status"], conflicts_with_all = ["automatic", "version", "install_dir", "container", "check_url", "force_fallback", "no_force_fallback", "no_web_assets", "no_incus_sync"])]
    pub auto_update: Option<String>,
    /// Release tag to install. Defaults to the latest selected-channel release with a Labby binary asset.
    #[arg(long, default_value = "latest")]
    pub version: String,
    /// Install directory for the host binary.
    #[arg(long)]
    pub install_dir: Option<PathBuf>,
    /// Do not sync the updated binary into an Incus container.
    #[arg(long)]
    pub no_incus_sync: bool,
    /// Do not update or clear the Incus filesystem web asset directory.
    #[arg(long)]
    pub no_web_assets: bool,
    /// Incus container name for the post-update sync.
    #[arg(long)]
    pub container: Option<String>,
    /// Optional public or host-bound URL to verify after the Incus sync.
    #[arg(long)]
    pub check_url: Option<String>,
    /// Fall back to `incus stop --force && incus start` if the service restart path fails.
    #[arg(long)]
    pub force_fallback: bool,
    /// Disable the default Incus force-restart fallback after installing the release.
    #[arg(long, conflicts_with = "force_fallback")]
    pub no_force_fallback: bool,
    /// Print what would happen without installing or syncing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, serde::Serialize)]
struct UpdateOutcome {
    channel: crate::self_update::UpdateChannel,
    version: String,
    install_dir: PathBuf,
    binary: PathBuf,
    dry_run: bool,
    installed: bool,
    incus_sync: Option<crate::dispatch::setup::incus::IncusSyncOutcome>,
    incus_sync_skipped: Option<String>,
}

pub async fn run(args: UpdateArgs, format: OutputFormat) -> Result<ExitCode> {
    if cfg!(windows) {
        anyhow::bail!(
            "labby host update is not supported on Windows; close Labby and rerun the verified PowerShell installer described at https://github.com/dinglebear-ai/labby#quick-start to update labby.exe"
        );
    }
    if let Some(action) = &args.auto_update {
        let outcome = crate::self_update::schedule(action, args.dry_run, args.channel)?;
        print(&outcome, format)?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.automatic {
        let outcome =
            crate::self_update::automatic(&std::env::current_exe()?, args.dry_run, args.channel)
                .await?;
        print(&outcome, format)?;
        return Ok(ExitCode::SUCCESS);
    }
    let channel = args.channel.unwrap_or_default();
    crate::self_update::validate_requested_release(&args.version, channel)?;
    let install_dir = resolve_install_dir(args.install_dir.as_ref())?;
    let binary = install_dir.join("labby");

    if args.dry_run {
        let outcome = UpdateOutcome {
            channel,
            version: args.version,
            install_dir,
            binary,
            dry_run: true,
            installed: false,
            incus_sync: None,
            incus_sync_skipped: if args.no_incus_sync {
                Some("--no-incus-sync requested".to_string())
            } else {
                None
            },
        };
        render_outcome(outcome, format)?;
        return Ok(ExitCode::SUCCESS);
    }

    crate::self_update::install_requested_release(&args.version, &install_dir, channel).await?;
    let (incus_sync, incus_sync_skipped) = if args.no_incus_sync {
        (None, Some("--no-incus-sync requested".to_string()))
    } else {
        match crate::dispatch::setup::incus::sync_incus_binary(
            crate::dispatch::setup::incus::IncusSyncOptions {
                container: args.container.clone(),
                binary: Some(binary.clone()),
                web_assets_dir: None,
                sync_web_assets: !args.no_web_assets,
                check_url: args.check_url,
                force_fallback: args.force_fallback || !args.no_force_fallback,
                dry_run: false,
                rollback: false,
            },
        ) {
            Ok(outcome) => (Some(outcome), None),
            Err(err)
                if should_skip_optional_incus_sync(
                    &err,
                    args.container.as_deref(),
                    std::env::var("LABBY_INCUS_CONTAINER").ok().as_deref(),
                ) =>
            {
                (None, Some(err.to_string()))
            }
            Err(err) => return Err(anyhow::anyhow!(err.to_string())),
        }
    };

    let outcome = UpdateOutcome {
        channel,
        version: args.version,
        install_dir,
        binary,
        dry_run: false,
        installed: true,
        incus_sync,
        incus_sync_skipped,
    };
    render_outcome(outcome, format)?;
    Ok(ExitCode::SUCCESS)
}

fn should_skip_optional_incus_sync(
    error: &crate::dispatch::error::ToolError,
    explicit_container: Option<&str>,
    configured_container: Option<&str>,
) -> bool {
    let target_requested = [explicit_container, configured_container]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty());
    !target_requested
        && matches!(
            error.kind(),
            "incus_client_missing" | "incus_sync_container_discovery_failed"
        )
}

fn resolve_install_dir(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }
    if let Some(path) = std::env::var_os("LABBY_INSTALL_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

fn render_outcome(outcome: UpdateOutcome, format: OutputFormat) -> Result<()> {
    if format.is_json() {
        print(&serde_json::to_value(outcome)?, format)?;
        return Ok(());
    }
    if outcome.dry_run {
        println!("dry-run: validated the requested Labby installation update");
    } else {
        println!("updated labby: {}", outcome.binary.display());
    }
    if let Some(sync) = &outcome.incus_sync {
        println!("synced Incus container: {}", sync.container);
        if let Some(version) = &sync.remote_version {
            println!("container version: {version}");
        }
    }
    if let Some(reason) = &outcome.incus_sync_skipped {
        println!("Incus sync skipped: {reason}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::error::ToolError;

    #[test]
    fn channel_arguments_default_stable_and_require_known_streams() {
        use clap::Parser;
        #[derive(Parser)]
        struct Fixture {
            #[command(flatten)]
            options: UpdateArgs,
        }
        assert_eq!(
            Fixture::try_parse_from(["labby"])
                .unwrap()
                .options
                .channel
                .unwrap_or_default(),
            crate::self_update::UpdateChannel::Stable
        );
        assert_eq!(
            Fixture::try_parse_from(["labby", "--channel", "dev"])
                .unwrap()
                .options
                .channel,
            Some(crate::self_update::UpdateChannel::Dev)
        );
        assert!(Fixture::try_parse_from(["labby", "--channel", "preview"]).is_err());
    }

    #[tokio::test]
    async fn dry_run_rejects_dev_tag_without_channel_opt_in() {
        use clap::Parser;
        #[derive(Parser)]
        struct Fixture {
            #[command(flatten)]
            options: UpdateArgs,
        }
        let args = Fixture::try_parse_from(["labby", "--version", "v1.2.3-dev.1", "--dry-run"])
            .unwrap()
            .options;
        assert!(
            run(
                args,
                OutputFormat::from_json_flag(
                    true,
                    crate::output::ColorPolicy::Plain,
                    crate::output::RenderEnv::stdout()
                )
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn dry_run_rejects_noncanonical_release_tags() {
        use clap::Parser;
        #[derive(Parser)]
        struct Fixture {
            #[command(flatten)]
            options: UpdateArgs,
        }
        let format = OutputFormat::from_json_flag(
            true,
            crate::output::ColorPolicy::Plain,
            crate::output::RenderEnv::stdout(),
        );
        for tag in ["1.2.3", "v1.2.3\n", " v1.2.3", "v1.2.3 ", "v1.2.3-dev.1\n"] {
            let args = Fixture::try_parse_from(["labby", "--version", tag, "--dry-run"])
                .unwrap()
                .options;
            assert!(run(args, format).await.is_err(), "{tag:?}");
        }
    }

    fn sync_error(kind: &str) -> ToolError {
        ToolError::Sdk {
            message: "fixture failure".into(),
            sdk_kind: kind.into(),
        }
    }

    #[test]
    fn optional_incus_sync_skips_missing_client_and_undiscovered_container() {
        for kind in [
            "incus_client_missing",
            "incus_sync_container_discovery_failed",
        ] {
            let error = sync_error(kind);
            assert!(should_skip_optional_incus_sync(&error, None, None));
            assert!(should_skip_optional_incus_sync(&error, Some(" "), Some("")));
        }
    }

    #[test]
    fn requested_incus_targets_remain_strict() {
        for kind in [
            "incus_client_missing",
            "incus_sync_container_discovery_failed",
        ] {
            let error = sync_error(kind);
            for target in ["labby", "remote:labby"] {
                assert!(!should_skip_optional_incus_sync(&error, Some(target), None));
                assert!(!should_skip_optional_incus_sync(&error, None, Some(target)));
            }
        }
    }

    #[test]
    fn optional_incus_sync_does_not_hide_other_failures() {
        for kind in [
            "incus_sync_list_failed",
            "incus_command_failed",
            "incus_sync_exec_failed",
            "incus_sync_binary_missing",
            "incus_sync_rolled_back",
        ] {
            assert!(!should_skip_optional_incus_sync(
                &sync_error(kind),
                None,
                None
            ));
        }
    }
}
