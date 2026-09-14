//! Bounded TLC adapter. Registration is metadata-only and performs no IO.

mod sha256;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io::{Read, Seek},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use verify_core::{
    Availability, Backend, BackendId, BackendReport, Bounds, Capabilities, CheckPlan, Kind, Verdict,
};

/// Required upstream TLA+ tools release.
pub const TLA_TOOLS_VERSION: &str = "1.7.4";
/// Observed SHA-256 of the official release jar.
pub const TLA_TOOLS_SHA256: &str =
    "936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88";
/// Immutable Apalache image reference; this adapter does not claim it ran.
pub const APALACHE_IMAGE: &str = "ghcr.io/apalache-mc/apalache@sha256:2003be7b0546c54be85adfc2969e43b28e4903f960e27c00dfdf1ed4f106113c";
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Metadata for one TLC module/config pair.
#[derive(Clone, Debug)]
pub struct TlaHarness {
    module: PathBuf,
    config: PathBuf,
}
impl TlaHarness {
    /// Construct a handle without reading either path.
    pub fn new(module: impl Into<PathBuf>, config: impl Into<PathBuf>) -> Result<Self, String> {
        let value = Self {
            module: module.into(),
            config: config.into(),
        };
        if value.module.as_os_str().is_empty() || value.config.as_os_str().is_empty() {
            Err("module and config paths must be nonempty".into())
        } else {
            Ok(value)
        }
    }
}

/// Exact-release TLC process adapter.
pub struct TlaBackend {
    java: PathBuf,
    jar: PathBuf,
    expected_sha256: String,
    harnesses: BTreeMap<(String, String), TlaHarness>,
}
impl TlaBackend {
    /// Configure paths without probing them.
    pub fn new(java: impl Into<PathBuf>, jar: impl Into<PathBuf>) -> Self {
        Self {
            java: java.into(),
            jar: jar.into(),
            expected_sha256: TLA_TOOLS_SHA256.into(),
            harnesses: BTreeMap::new(),
        }
    }
    /// Override the expected artifact identity, primarily for private mirrors and fixtures.
    pub fn with_expected_sha256(mut self, digest: impl Into<String>) -> Self {
        self.expected_sha256 = digest.into();
        self
    }
    /// Register metadata without executing TLC.
    pub fn register(
        &mut self,
        model: impl Into<String>,
        handle: impl Into<String>,
        harness: TlaHarness,
    ) -> Result<(), String> {
        let key = (model.into(), handle.into());
        if key.0.trim().is_empty() || key.1.trim().is_empty() {
            return Err("model and handle must be nonempty".into());
        }
        match self.harnesses.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(harness);
            }
            Entry::Occupied(_) => return Err("duplicate TLC model/handle".into()),
        }
        Ok(())
    }
    fn report(
        &self,
        plan: &CheckPlan,
        verdict: Verdict,
        ran: bool,
        scenarios: Vec<serde_json::Value>,
    ) -> BackendReport {
        BackendReport {
            backend: self.id(),
            invariant: plan.invariant.clone(),
            verdict,
            tool_version: ran.then(|| TLA_TOOLS_VERSION.into()),
            scenarios,
        }
    }
    fn probe(&self, timeout: Duration) -> Result<(), String> {
        let actual = sha256::file(&self.jar)?;
        if actual != self.expected_sha256 {
            return Err("TLC jar SHA-256 mismatch".into());
        }
        let output = execute(
            &self.java,
            &[
                "-cp".into(),
                self.jar.as_os_str().into(),
                "tlc2.TLC".into(),
                "-help".into(),
            ],
            timeout,
        )?;
        let text = output.text();
        // TLC 1.7.4 prints its version-bearing help and exits 1. Accept only
        // that release's observed help statuses, never a signal/unknown exit.
        if matches!(output.status, Some(0 | 1)) && text.contains("Version 2.19") {
            Ok(())
        } else {
            Err(format!(
                "TLC {TLA_TOOLS_VERSION} unavailable or version mismatch"
            ))
        }
    }
}
impl Backend for TlaBackend {
    fn id(&self) -> BackendId {
        "tla".to_owned().try_into().expect("valid id")
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kinds: BTreeSet::from([Kind::Safety, Kind::Security]),
            fairness: false,
            concurrency: false,
            bounded: true,
        }
    }
    fn has_handle(&self, model: &str, handle: &str) -> bool {
        self.harnesses.contains_key(&(model.into(), handle.into()))
    }
    fn availability(&self) -> Availability {
        self.probe(Duration::from_secs(5)).map_or_else(
            |reason| Availability::Missing { reason },
            |_| Availability::Ready {},
        )
    }
    fn run(&self, plan: &CheckPlan) -> BackendReport {
        let deadline = Instant::now() + Duration::from_millis(plan.timeout_ms.get());
        let Some(harness) = self
            .harnesses
            .get(&(plan.model.clone(), plan.handle.clone()))
        else {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "unregistered TLC model/handle".into(),
                },
                false,
                vec![],
            );
        };
        // TLC's -depth controls random simulation, not model checking. Reject
        // unsupported limits before probing so they cannot appear effective.
        let Some(workers) = exact_positive(&plan.bounds, "workers") else {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "TLC model checking requires exactly a positive workers bound".into(),
                },
                false,
                vec![],
            );
        };
        if plan.bounds.len() != 1 || plan.seed.is_some() {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "TLC model checking accepts only workers and no seed; depth is a simulation-only option".into(),
                },
                false,
                vec![],
            );
        }
        if let Err(reason) = self.probe(deadline.saturating_duration_since(Instant::now())) {
            if Instant::now() >= deadline {
                return self.report(
                    plan,
                    Verdict::Incomplete {
                        reason: "TLC deadline exceeded during integrity/version probe".into(),
                        explored: Bounds::new(),
                    },
                    false,
                    vec![],
                );
            }
            return self.report(plan, Verdict::Skipped { reason }, false, vec![]);
        }
        let mut execution_scope = plan.bounds.clone();
        execution_scope.insert("timeout_ms".into(), plan.timeout_ms.get().into());
        execution_scope.insert("scope".into(), "registered_module_and_config".into());
        let args = vec![
            "-cp".into(),
            self.jar.as_os_str().into(),
            "tlc2.TLC".into(),
            "-workers".into(),
            workers.to_string().into(),
            "-config".into(),
            harness.config.as_os_str().into(),
            harness.module.as_os_str().into(),
        ];
        match execute(
            &self.java,
            &args,
            deadline.saturating_duration_since(Instant::now()),
        ) {
            Err(reason) => self.report(plan, Verdict::Error { reason }, true, vec![]),
            Ok(output) if output.timed_out => self.report(
                plan,
                Verdict::Incomplete {
                    reason: "TLC deadline exceeded".into(),
                    explored: execution_scope.clone(),
                },
                true,
                vec![],
            ),
            Ok(output) if output.limited => self.report(
                plan,
                Verdict::Incomplete {
                    reason: format!("TLC output exceeded {MAX_OUTPUT} bytes"),
                    explored: execution_scope.clone(),
                },
                true,
                vec![],
            ),
            Ok(output) => {
                let text = output.text();
                if output.status == Some(12) && tlc_invariant_violation(&text) {
                    self.report(
                        plan,
                        Verdict::Falsified {
                            reason: "TLC reported an invariant violation; its native state trace has no project-specific scenario projector".into(),
                        },
                        true,
                        vec![],
                    )
                } else if output.status != Some(0) {
                    self.report(
                        plan,
                        Verdict::Error {
                            reason: "TLC execution failed".into(),
                        },
                        true,
                        vec![],
                    )
                } else if output.status == Some(0)
                    && text.contains("Model checking completed. No error has been found")
                {
                    self.report(
                        plan,
                        Verdict::Bounded {
                            bounds: execution_scope.clone(),
                        },
                        true,
                        vec![],
                    )
                } else {
                    self.report(
                        plan,
                        Verdict::Error {
                            reason: format!(
                                "TLC exited without a recognized result ({})",
                                output
                                    .status
                                    .map_or_else(|| "signal".into(), |v| v.to_string())
                            ),
                        },
                        true,
                        vec![],
                    )
                }
            }
        }
    }
}

fn exact_positive(bounds: &Bounds, key: &str) -> Option<u64> {
    bounds.get(key)?.as_u64().filter(|v| *v > 0)
}
struct Output {
    status: Option<i32>,
    bytes: Vec<u8>,
    timed_out: bool,
    limited: bool,
}
impl Output {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}
fn execute(
    program: &PathBuf,
    args: &[std::ffi::OsString],
    timeout: Duration,
) -> Result<Output, String> {
    let file = tempfile::tempfile().map_err(|e| format!("create output sink: {e}"))?;
    let stderr = file
        .try_clone()
        .map_err(|e| format!("clone output sink: {e}"))?;
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| format!("start process: {e}"))?;
    let start = Instant::now();
    let mut timed_out = false;
    let mut limited = false;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s.code();
        }
        if start.elapsed() >= timeout {
            timed_out = true;
            terminate_tree(&mut child);
            break child.wait().map_err(|e| e.to_string())?.code();
        }
        if file.metadata().map(|metadata| metadata.len()).unwrap_or(0) > MAX_OUTPUT as u64 {
            limited = true;
            terminate_tree(&mut child);
            break child.wait().map_err(|e| e.to_string())?.code();
        }
        thread::sleep(Duration::from_millis(5));
    };
    // The leader may exit after forking a child that still owns verifier IO.
    // Reap the isolated group on every result path, not only timeout paths.
    terminate_tree(&mut child);
    let mut output_file = file;
    output_file.rewind().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    output_file
        .take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    limited |= bytes.len() > MAX_OUTPUT;
    bytes.truncate(MAX_OUTPUT);
    Ok(Output {
        status,
        bytes,
        timed_out,
        limited,
    })
}
fn tlc_invariant_violation(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("Error: Invariant ") && line.ends_with(" is violated."))
}
fn terminate_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            // A negative PGID must be an operand, not another signal option.
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}
