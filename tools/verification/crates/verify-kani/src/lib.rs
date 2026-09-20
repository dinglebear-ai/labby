//! Domain-neutral adapter for finite Kani proof harnesses.
//!
//! Registration is metadata-only: it neither probes the configured executable
//! nor resolves harness source paths. Execution is always bounded by an exact
//! unwind value, a combined output ceiling, and the runner-supplied deadline.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
#[cfg(unix)]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use verify_core::{
    Availability, Backend, BackendId, BackendReport, Bounds, Capabilities, CheckPlan, Kind, Verdict,
};

const BACKEND_ID: &str = "kani";
const KANI_VERSION: &str = "0.67.0";
const VERSION_OUTPUT_LIMIT: usize = 4 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(unix)]
const READ_CHUNK_BYTES: usize = 4 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_UNWIND: u32 = 1_000_000;
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(unix)]
const TERMINATION_GRACE: Duration = Duration::from_millis(100);

/// Metadata needed to invoke one project-owned proof harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KaniHarness {
    source: PathBuf,
    symbol: String,
}

impl KaniHarness {
    /// Describe a standalone Rust source and its Kani proof symbol.
    pub fn new(source: impl Into<PathBuf>, symbol: impl Into<String>) -> Result<Self, String> {
        let source = source.into();
        let symbol = symbol.into();
        if source.as_os_str().is_empty() {
            return Err("Kani harness source must be nonempty".into());
        }
        if symbol.trim().is_empty() {
            return Err("Kani harness symbol must be nonempty".into());
        }
        Ok(Self { source, symbol })
    }

    /// Source path passed to the verifier only when a check is executed.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Harness filter passed to the verifier.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
}

/// Exact-version Kani adapter with a metadata-only harness registry.
pub struct KaniBackend {
    executable: PathBuf,
    harnesses: BTreeMap<(String, String), KaniHarness>,
}

impl KaniBackend {
    /// Configure an adapter using `kani-driver` from `PATH`.
    pub fn from_path() -> Self {
        Self::new("kani-driver")
    }

    /// Configure an explicit executable without probing or installing it.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            harnesses: BTreeMap::new(),
        }
    }

    /// Register a project model/handle mapping without filesystem or tool access.
    pub fn register(
        &mut self,
        model: impl Into<String>,
        handle: impl Into<String>,
        harness: KaniHarness,
    ) -> Result<(), String> {
        let key = (model.into(), handle.into());
        if key.0.trim().is_empty() || key.1.trim().is_empty() {
            return Err("model and handle must be nonempty".into());
        }
        match self.harnesses.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "Kani harness already registered: {}/{}",
                    entry.key().0,
                    entry.key().1
                ));
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(harness);
            }
        }
        Ok(())
    }

    fn report(
        &self,
        plan: &CheckPlan,
        verdict: Verdict,
        tool_version: Option<String>,
    ) -> BackendReport {
        BackendReport {
            backend: self.id(),
            invariant: plan.invariant.clone(),
            verdict,
            tool_version,
            scenarios: Vec::new(),
        }
    }

    fn probe(&self) -> Result<(), String> {
        let output = run_bounded(
            &self.executable,
            [OsStr::new("--version")],
            VERSION_TIMEOUT,
            VERSION_OUTPUT_LIMIT,
        )
        .map_err(|error| format!("Kani {KANI_VERSION} unavailable: {error}"))?;
        if output.timed_out {
            return Err(format!(
                "Kani {KANI_VERSION} version probe exceeded {} ms",
                VERSION_TIMEOUT.as_millis()
            ));
        }
        if output.output_limited {
            return Err(format!(
                "Kani {KANI_VERSION} version probe exceeded {VERSION_OUTPUT_LIMIT} output bytes"
            ));
        }
        if !output.status.success() {
            return Err(format!(
                "Kani {KANI_VERSION} version probe exited {}",
                display_status(output.status)
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let observed = stdout
            .trim()
            .strip_prefix("kani ")
            .or_else(|| stderr.trim().strip_prefix("kani "));
        match observed {
            Some(KANI_VERSION) => Ok(()),
            Some(version) => Err(format!(
                "Kani version mismatch: required {KANI_VERSION}, observed {version}"
            )),
            None => Err(format!(
                "Kani version mismatch: required exact output `kani {KANI_VERSION}`"
            )),
        }
    }
}

impl Default for KaniBackend {
    fn default() -> Self {
        Self::from_path()
    }
}

impl Backend for KaniBackend {
    fn id(&self) -> BackendId {
        BackendId::try_from(BACKEND_ID.to_owned()).expect("constant backend id")
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kinds: BTreeSet::from([Kind::Safety, Kind::Security, Kind::Refinement]),
            fairness: false,
            concurrency: false,
            bounded: true,
        }
    }

    fn has_handle(&self, model: &str, handle: &str) -> bool {
        self.harnesses
            .contains_key(&(model.to_owned(), handle.to_owned()))
    }

    fn availability(&self) -> Availability {
        match self.probe() {
            Ok(()) => Availability::Ready {},
            Err(reason) => Availability::Missing { reason },
        }
    }

    fn run(&self, plan: &CheckPlan) -> BackendReport {
        let limits = match Limits::parse(&plan.bounds) {
            Ok(limits) => limits,
            Err(reason) => return self.report(plan, Verdict::Error { reason }, None),
        };
        if plan.seed.is_some() {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "Kani does not accept a sampling seed".into(),
                },
                None,
            );
        }
        let Some(harness) = self
            .harnesses
            .get(&(plan.model.clone(), plan.handle.clone()))
        else {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "unregistered Kani model/handle".into(),
                },
                None,
            );
        };
        if let Availability::Missing { reason } = self.availability() {
            return self.report(plan, Verdict::Skipped { reason }, None);
        }

        let unwind = limits.unwind.to_string();
        let arguments = [
            OsStr::new("--output-format"),
            OsStr::new("regular"),
            OsStr::new("--unwind"),
            OsStr::new(&unwind),
            OsStr::new("--exact"),
            OsStr::new("--harness"),
            OsStr::new(harness.symbol()),
            harness.source().as_os_str(),
        ];
        let timeout = Duration::from_millis(plan.timeout_ms.get());
        let output = match run_bounded(
            &self.executable,
            arguments,
            timeout,
            limits.max_output_bytes,
        ) {
            Ok(output) => output,
            Err(reason) => {
                return self.report(
                    plan,
                    Verdict::Error {
                        reason: format!("failed to invoke Kani: {reason}"),
                    },
                    Some(KANI_VERSION.into()),
                );
            }
        };
        let explored = limits.reported(plan.timeout_ms.get());
        if output.timed_out {
            return self.report(
                plan,
                Verdict::Incomplete {
                    reason: format!("Kani deadline exceeded after {} ms", plan.timeout_ms),
                    explored,
                },
                Some(KANI_VERSION.into()),
            );
        }
        if output.output_limited {
            return self.report(
                plan,
                Verdict::Incomplete {
                    reason: format!(
                        "Kani output exceeded the {} byte limit",
                        limits.max_output_bytes
                    ),
                    explored,
                },
                Some(KANI_VERSION.into()),
            );
        }

        let combined = bounded_diagnostic(&output.stdout, &output.stderr);
        let verdict = interpret_output(output.status, &combined, explored);
        self.report(plan, verdict, Some(KANI_VERSION.into()))
    }
}

#[derive(Clone, Copy, Debug)]
struct Limits {
    unwind: u32,
    max_output_bytes: usize,
}

impl Limits {
    fn parse(bounds: &Bounds) -> Result<Self, String> {
        const KEYS: [&str; 2] = ["max_output_bytes", "unwind"];
        if bounds.len() != KEYS.len() || KEYS.iter().any(|key| !bounds.contains_key(*key)) {
            return Err("bounds must contain exactly max_output_bytes and unwind".into());
        }
        let unwind = bounds["unwind"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "bound unwind must be a positive u32".to_owned())?;
        if unwind > MAX_UNWIND {
            return Err(format!("bound unwind must not exceed {MAX_UNWIND}"));
        }
        let max_output_bytes = bounds["max_output_bytes"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                "bound max_output_bytes must be a positive platform-sized integer".to_owned()
            })?;
        if max_output_bytes > MAX_OUTPUT_BYTES {
            return Err(format!(
                "bound max_output_bytes must not exceed {MAX_OUTPUT_BYTES}"
            ));
        }
        Ok(Self {
            unwind,
            max_output_bytes,
        })
    }

    fn reported(self, timeout_ms: u64) -> Bounds {
        BTreeMap::from([
            (
                "max_output_bytes".into(),
                serde_json::json!(self.max_output_bytes),
            ),
            ("timeout_ms".into(), serde_json::json!(timeout_ms)),
            ("unwind".into(), serde_json::json!(self.unwind)),
        ])
    }
}

fn interpret_output(status: ExitStatus, output: &str, bounds: Bounds) -> Verdict {
    if has_failed_unwind(output) {
        return Verdict::Incomplete {
            reason: "Kani reached the declared unwind bound".into(),
            explored: bounds,
        };
    }
    if missing_prerequisite(output) {
        return Verdict::Skipped {
            reason: "Kani proof prerequisite unavailable".into(),
        };
    }
    if !status.success()
        && output.contains("VERIFICATION:- FAILED")
        && output.contains("Status: FAILURE")
    {
        return Verdict::Falsified {
            reason: "Kani found a property violation; no scenario projector is registered".into(),
        };
    }
    if status.success()
        && output.contains("VERIFICATION:- SUCCESSFUL")
        && output.contains("successfully verified harnesses")
    {
        return Verdict::Bounded { bounds };
    }
    Verdict::Error {
        reason: format!(
            "Kani exited {} without a complete proof result",
            display_status(status)
        ),
    }
}

fn missing_prerequisite(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    let missing = lower.contains("not found")
        || lower.contains("no such file or directory")
        || lower.contains("toolchain is not installed")
        || lower.contains("failed to spawn")
        || lower.contains("failed to invoke");
    missing && (lower.contains("cbmc") || lower.contains("goto-cc") || lower.contains("toolchain"))
}

fn has_failed_unwind(output: &str) -> bool {
    let lines: Vec<_> = output.lines().collect();
    lines.iter().enumerate().any(|(index, line)| {
        line.contains("Status: FAILURE")
            && lines.iter().skip(index + 1).take(3).any(|detail| {
                let detail = detail.to_ascii_lowercase();
                detail.contains("unwinding assertion") || detail.contains("unwinding check")
            })
    })
}

fn bounded_diagnostic(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    if !combined.is_empty() && !stderr.is_empty() {
        combined.push('\n');
    }
    combined.push_str(&String::from_utf8_lossy(stderr));
    combined
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    output_limited: bool,
}

#[cfg(unix)]
fn run_bounded<I, S>(
    executable: &Path,
    arguments: I,
    timeout: Duration,
    output_limit: usize,
) -> Result<ProcessOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(executable);
    // Standalone Kani bundles spawn `goto-cc`, `cbmc`, and sibling tools by
    // name. An explicit driver path must therefore carry its verified bundle
    // directory into the child PATH instead of depending on caller ambience.
    if let Some(parent) = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let inherited_path = std::env::var_os("PATH");
        let paths = std::iter::once(parent.to_path_buf()).chain(
            inherited_path
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten(),
        );
        let path = std::env::join_paths(paths)
            .map_err(|error| format!("construct Kani bundle PATH: {error}"))?;
        command.env("PATH", path);
    }
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    // Independent socket endpoints let the parent read without blocking while
    // the verifier retains ordinary blocking output. No reader threads can be
    // stranded by inherited descriptors after the process-group leader exits.
    let (mut stdout, stdout_child) = UnixStream::pair().map_err(|error| error.to_string())?;
    let (mut stderr, stderr_child) = UnixStream::pair().map_err(|error| error.to_string())?;
    stdout
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    stderr
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(stdout_child)))
        .stderr(Stdio::from(OwnedFd::from(stderr_child)));
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    // Command retains configured output handles after spawn. Release the
    // parent's copies so EOF reflects only handles held by child processes.
    drop(command);
    let started = Instant::now();
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    let mut timed_out = false;
    let mut output_limited = false;
    let mut status = None;
    let result = loop {
        // Read at most one chunk per stream before checking time and output
        // budgets; a continuously writing verifier cannot starve these checks.
        if let Err(error) = read_chunk(
            &mut stdout,
            &mut stdout_bytes,
            stderr_bytes.len(),
            &mut stdout_closed,
            output_limit,
            &mut output_limited,
        ) {
            break Err(error);
        }
        if let Err(error) = read_chunk(
            &mut stderr,
            &mut stderr_bytes,
            stdout_bytes.len(),
            &mut stderr_closed,
            output_limit,
            &mut output_limited,
        ) {
            break Err(error);
        }
        if output_limited || started.elapsed() >= timeout {
            timed_out = !output_limited;
            break Ok(());
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exited)) => {
                    status = Some(exited);
                    cleanup_process_group(child.id());
                }
                Ok(None) => {}
                Err(error) => break Err(error.to_string()),
            }
        }
        if status.is_some() && stdout_closed && stderr_closed {
            break Ok(());
        }
        thread::sleep(POLL_INTERVAL);
    };
    if status.is_none() {
        terminate_process_group(&mut child);
        status = Some(child.wait().map_err(|error| error.to_string())?);
    }
    result?;
    // Closing the receivers is bounded even when a detached descendant still
    // holds output handles. Timeout reports no claim that such a child was reaped.
    Ok(ProcessOutput {
        status: status.expect("leader was reaped"),
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        timed_out,
        output_limited,
    })
}

#[cfg(unix)]
fn read_chunk(
    reader: &mut impl Read,
    bytes: &mut Vec<u8>,
    other_len: usize,
    closed: &mut bool,
    limit: usize,
    output_limited: &mut bool,
) -> Result<(), String> {
    if *closed {
        return Ok(());
    }
    let mut buffer = [0; READ_CHUNK_BYTES];
    match reader.read(&mut buffer) {
        Ok(0) => *closed = true,
        Ok(count) => {
            let remaining = limit.saturating_sub(bytes.len().saturating_add(other_len));
            *output_limited |= count > remaining;
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

#[cfg(not(unix))]
fn run_bounded<I, S>(_: &Path, _: I, _: Duration, _: usize) -> Result<ProcessOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Err("bounded Kani subprocess execution requires Unix output descriptors".into())
}

fn display_status(status: ExitStatus) -> String {
    status.code().map_or_else(
        || "after a signal".into(),
        |code| format!("with code {code}"),
    )
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child) {
    let group = format!("-{}", child.id());
    signal_process_group("-TERM", &group);
    let started = Instant::now();
    while started.elapsed() < TERMINATION_GRACE {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    signal_process_group("-KILL", &group);
    let _ = child.kill();
}

#[cfg(unix)]
fn cleanup_process_group(id: u32) {
    signal_process_group("-KILL", &format!("-{id}"));
}

#[cfg(unix)]
fn signal_process_group(signal: &str, group: &str) {
    let _ = Command::new("kill")
        // A negative PGID must be an operand, not another signal option.
        .args([signal, "--", group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatus::from_raw(code as u32)
        }
    }

    fn bounds() -> Bounds {
        BTreeMap::from([
            ("max_output_bytes".into(), serde_json::json!(65_536)),
            ("unwind".into(), serde_json::json!(8)),
        ])
    }

    #[test]
    fn parser_never_promotes_a_completed_kani_proof_to_verified() {
        let verdict = interpret_output(
            status(0),
            "VERIFICATION:- SUCCESSFUL\nComplete - 1 successfully verified harnesses, 0 failures, 1 total.",
            bounds(),
        );
        assert!(matches!(verdict, Verdict::Bounded { .. }));
    }

    #[test]
    fn parser_distinguishes_property_failure_from_unwind_exhaustion() {
        let failed = interpret_output(
            status(1),
            "Status: FAILURE\nVERIFICATION:- FAILED\n1 failures, 1 total.",
            bounds(),
        );
        assert!(matches!(failed, Verdict::Falsified { .. }));

        let incomplete = interpret_output(
            status(1),
            "Status: FAILURE\nDescription: unwinding assertion loop 1\nVERIFICATION:- FAILED",
            bounds(),
        );
        assert!(matches!(incomplete, Verdict::Incomplete { .. }));

        let failed_with_successful_unwind = interpret_output(
            status(1),
            "Status: SUCCESS\nDescription: unwinding assertion loop 1\nStatus: FAILURE\nDescription: assertion failed\nVERIFICATION:- FAILED",
            bounds(),
        );
        assert!(matches!(
            failed_with_successful_unwind,
            Verdict::Falsified { .. }
        ));
    }

    #[test]
    fn parser_does_not_turn_compiler_failure_into_a_counterexample() {
        let verdict = interpret_output(
            status(1),
            "error[E0308]: mismatched types\nManual Harness Summary: 0 total",
            bounds(),
        );
        assert!(matches!(verdict, Verdict::Error { .. }));
    }

    #[test]
    fn parser_reports_an_absent_cbmc_prerequisite_as_skipped() {
        let verdict = interpret_output(
            status(1),
            "error: failed to invoke CBMC: No such file or directory",
            bounds(),
        );
        assert!(matches!(verdict, Verdict::Skipped { .. }));

        let verdict = interpret_output(
            status(1),
            "error: Failed to invoke goto-cc: No such file or directory",
            bounds(),
        );
        assert!(matches!(verdict, Verdict::Skipped { .. }));
    }

    #[test]
    fn limits_are_explicit_positive_and_closed_to_unknown_keys() {
        assert!(Limits::parse(&bounds()).is_ok());
        let mut unknown = bounds();
        unknown.insert("depth".into(), serde_json::json!(1));
        assert!(Limits::parse(&unknown).is_err());
        let mut zero = bounds();
        zero.insert("unwind".into(), serde_json::json!(0));
        assert!(Limits::parse(&zero).is_err());
        let mut excessive_output = bounds();
        excessive_output.insert(
            "max_output_bytes".into(),
            serde_json::json!(MAX_OUTPUT_BYTES + 1),
        );
        assert!(Limits::parse(&excessive_output).is_err());
    }
}
