//! Configuration defaults and bounded artifact retention controls.
/// Default per-artifact content cap, in MiB.
///
/// This is NOT a context guard — artifact content is written to disk and only
/// the small receipt is returned to the model. It is a resource bound that keeps
/// a single write comfortably under the runner's 64 MiB JS heap (see
/// `runner.rs`), so an oversized artifact fails as a clean `invalid_param`
/// instead of an opaque QuickJS out-of-memory trap. Override with
/// `LABBY_CODE_MODE_ARTIFACT_MAX_MIB` (keep it below ~64 to preserve the clean
/// error boundary).
const DEFAULT_ARTIFACT_MAX_MIB: usize = 8;

/// Default number of artifact run directories retained per owner under
/// `$LABBY_HOME/code-mode-artifacts/`. Routine retention is coalesced per run;
/// store-byte pressure triggers an additional pass during admission. Search and
/// no-write runs do not prune, and active runs remain protected.
/// Override with `LABBY_CODE_MODE_ARTIFACT_RETENTION_RUNS`;
/// set it to `0` to disable *count* pruning.
const DEFAULT_ARTIFACT_RETENTION_RUNS: usize = 200;

/// Default total-store byte budget, in MiB. Now that a single artifact can be
/// several MiB, the run-count cap alone no longer bounds disk usage, so pruning
/// also drops this owner’s oldest inactive runs to make room. Payload
/// admission rejects writes if protected runs leave insufficient space;
/// retrieval metadata is excluded from payload accounting. Override with `LABBY_CODE_MODE_ARTIFACT_MAX_STORE_MIB`; set it to
/// `0` to disable *byte* pruning.
const DEFAULT_ARTIFACT_MAX_STORE_MIB: u64 = 4096;

use crate::util::env_non_empty;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static ARTIFACT_RETENTION_RUNS_CONFIG_DEFAULT: AtomicUsize =
    AtomicUsize::new(DEFAULT_ARTIFACT_RETENTION_RUNS);
static ARTIFACT_MAX_MIB_CONFIG_DEFAULT: AtomicUsize = AtomicUsize::new(DEFAULT_ARTIFACT_MAX_MIB);
static ARTIFACT_MAX_STORE_MIB_CONFIG_DEFAULT: AtomicU64 =
    AtomicU64::new(DEFAULT_ARTIFACT_MAX_STORE_MIB);

/// Refresh the gateway artifact limits used when environment overrides are absent.
pub fn install_artifact_config_defaults(
    retention_runs: Option<usize>,
    max_mib: Option<usize>,
    max_store_mib: Option<u64>,
) {
    ARTIFACT_RETENTION_RUNS_CONFIG_DEFAULT.store(
        retention_runs.unwrap_or(DEFAULT_ARTIFACT_RETENTION_RUNS),
        Ordering::Relaxed,
    );
    ARTIFACT_MAX_MIB_CONFIG_DEFAULT.store(
        max_mib
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_ARTIFACT_MAX_MIB),
        Ordering::Relaxed,
    );
    ARTIFACT_MAX_STORE_MIB_CONFIG_DEFAULT.store(
        max_store_mib.unwrap_or(DEFAULT_ARTIFACT_MAX_STORE_MIB),
        Ordering::Relaxed,
    );
}

/// Resolve the per-run artifact retention cap from the environment, falling back
/// to `config.toml` then [`DEFAULT_ARTIFACT_RETENTION_RUNS`]. `0` disables pruning.
#[must_use]
pub(crate) fn artifact_retention_runs() -> usize {
    // Absent/blank → config.toml, then default silently. Present-but-unparseable
    // → warn and fall back, so a fat-fingered value (e.g. `5O`) isn't silently
    // ignored.
    let Some(raw) = env_non_empty("LABBY_CODE_MODE_ARTIFACT_RETENTION_RUNS") else {
        return ARTIFACT_RETENTION_RUNS_CONFIG_DEFAULT.load(Ordering::Relaxed);
    };
    match raw.trim().parse::<usize>() {
        Ok(value) => value,
        Err(_) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                value = %raw,
                default = DEFAULT_ARTIFACT_RETENTION_RUNS,
                "ignoring unparseable LABBY_CODE_MODE_ARTIFACT_RETENTION_RUNS; using default"
            );
            DEFAULT_ARTIFACT_RETENTION_RUNS
        }
    }
}

/// Resolve the per-artifact content cap (in bytes) from the environment,
/// falling back to [`DEFAULT_ARTIFACT_MAX_MIB`]. The env value is expressed in
/// MiB for ergonomics (`LABBY_CODE_MODE_ARTIFACT_MAX_MIB=16`).
#[must_use]
pub(crate) fn artifact_max_bytes() -> usize {
    let default_bytes = ARTIFACT_MAX_MIB_CONFIG_DEFAULT
        .load(Ordering::Relaxed)
        .saturating_mul(1024 * 1024);
    // Absent/blank → config.toml, then default silently. Present-but-unparseable
    // or `0` → warn and fall back (a 0 MiB cap would reject every write).
    let Some(raw) = env_non_empty("LABBY_CODE_MODE_ARTIFACT_MAX_MIB") else {
        return default_bytes;
    };
    match raw.trim().parse::<usize>() {
        Ok(mib) if mib > 0 => mib.saturating_mul(1024 * 1024),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                value = %raw,
                default_mib = DEFAULT_ARTIFACT_MAX_MIB,
                "ignoring invalid LABBY_CODE_MODE_ARTIFACT_MAX_MIB; using default"
            );
            default_bytes
        }
    }
}

/// Resolve the total-store byte budget from the environment, falling back to
/// [`DEFAULT_ARTIFACT_MAX_STORE_MIB`]. The env value is in MiB
/// (`LABBY_CODE_MODE_ARTIFACT_MAX_STORE_MIB=8192`); `0` disables byte pruning.
#[must_use]
pub(crate) fn artifact_max_store_bytes() -> u64 {
    let default_bytes = ARTIFACT_MAX_STORE_MIB_CONFIG_DEFAULT
        .load(Ordering::Relaxed)
        .saturating_mul(1024 * 1024);
    let Some(raw) = env_non_empty("LABBY_CODE_MODE_ARTIFACT_MAX_STORE_MIB") else {
        return default_bytes;
    };
    match raw.trim().parse::<u64>() {
        // `0` is meaningful here (disable byte pruning), unlike the per-artifact
        // cap where 0 is nonsense.
        Ok(mib) => mib.saturating_mul(1024 * 1024),
        Err(_) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                value = %raw,
                default_mib = DEFAULT_ARTIFACT_MAX_STORE_MIB,
                "ignoring unparseable LABBY_CODE_MODE_ARTIFACT_MAX_STORE_MIB; using default"
            );
            default_bytes
        }
    }
}
