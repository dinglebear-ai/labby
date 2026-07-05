//! `labby health` — quick reachability ping for every configured service.

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// One row of the health report.
#[derive(Debug, Clone, Serialize)]
pub struct HealthRow {
    pub service: String,
    pub reachable: bool,
    pub auth_ok: bool,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub message: Option<String>,
}

/// Run the health subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let rows: Vec<HealthRow> = Vec::new();

    let any_unhealthy = rows.iter().any(|r| !r.reachable || !r.auth_ok);
    print(&rows, format)?;
    if any_unhealthy {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
