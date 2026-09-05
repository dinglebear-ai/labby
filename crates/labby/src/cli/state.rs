use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct StateArgs {
    #[command(subcommand)]
    pub command: StateCommand,
}

#[derive(Debug, Subcommand)]
pub enum StateCommand {
    /// Export an authenticated disaster-recovery bundle using LABBY_RECOVERY_KEY_PATH.
    Export {
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify a bundle's HMAC, schema, compatibility, paths, sizes, and digests.
    Verify {
        #[arg(long)]
        bundle: PathBuf,
    },
    /// Restore an authenticated bundle offline using LABBY_RECOVERY_KEY_PATH.
    Restore {
        #[arg(long)]
        bundle: PathBuf,
    },
}

#[allow(clippy::print_stdout)]
#[derive(Serialize)]
struct StateOutcome {
    operation: &'static str,
    committed: bool,
    entries_verified: usize,
    manifest_version: u32,
    maintenance_warning: Option<String>,
}

pub fn run(args: StateArgs, format: OutputFormat) -> Result<ExitCode> {
    let (operation, committed, manifest, maintenance_warning) = match args.command {
        StateCommand::Export { output } => (
            "export",
            true,
            crate::durable_state::export_bundle(&output)?,
            None,
        ),
        StateCommand::Verify { bundle } => (
            "verify",
            false,
            crate::durable_state::verify_bundle(&bundle)?,
            None,
        ),
        StateCommand::Restore { bundle } => {
            let outcome = crate::durable_state::restore_bundle(&bundle)?;
            (
                "restore",
                true,
                outcome.manifest,
                outcome.maintenance_warning,
            )
        }
    };
    let outcome = StateOutcome {
        operation,
        committed,
        entries_verified: manifest.entries.len(),
        manifest_version: manifest.manifest_version,
        maintenance_warning,
    };
    if format.is_json() {
        print(&outcome, format)?;
    } else {
        println!(
            "{} durable-state entries verified (manifest v{})",
            outcome.entries_verified, outcome.manifest_version
        );
        if let Some(warning) = &outcome.maintenance_warning {
            eprintln!("restore committed with maintenance warning: {warning}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
