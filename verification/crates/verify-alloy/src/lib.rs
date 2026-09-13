//! Bounded Alloy adapter. Registration is metadata-only and performs no IO.

mod sha256;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
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

/// Required Alloy release.
pub const ALLOY_VERSION: &str = "6.2.0";
/// Observed SHA-256 of the official release jar.
pub const ALLOY_SHA256: &str = "6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d";
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Metadata for one command in an Alloy module.
#[derive(Clone, Debug)]
pub struct AlloyHarness {
    module: PathBuf,
    command: String,
}
impl AlloyHarness {
    /// Construct a handle without reading the module.
    pub fn new(module: impl Into<PathBuf>, command: impl Into<String>) -> Result<Self, String> {
        let value = Self {
            module: module.into(),
            command: command.into(),
        };
        if value.module.as_os_str().is_empty() || value.command.trim().is_empty() {
            Err("module path and command must be nonempty".into())
        } else {
            Ok(value)
        }
    }
}

/// Exact-release Alloy process adapter.
pub struct AlloyBackend {
    java: PathBuf,
    jar: PathBuf,
    expected_sha256: String,
    harnesses: BTreeMap<(String, String), AlloyHarness>,
}
impl AlloyBackend {
    /// Configure paths without probing them.
    pub fn new(java: impl Into<PathBuf>, jar: impl Into<PathBuf>) -> Self {
        Self {
            java: java.into(),
            jar: jar.into(),
            expected_sha256: ALLOY_SHA256.into(),
            harnesses: BTreeMap::new(),
        }
    }
    /// Override the expected artifact identity, primarily for private mirrors and fixtures.
    pub fn with_expected_sha256(mut self, digest: impl Into<String>) -> Self {
        self.expected_sha256 = digest.into();
        self
    }
    /// Register metadata without executing Alloy.
    pub fn register(
        &mut self,
        model: impl Into<String>,
        handle: impl Into<String>,
        harness: AlloyHarness,
    ) -> Result<(), String> {
        let key = (model.into(), handle.into());
        if key.0.trim().is_empty() || key.1.trim().is_empty() {
            return Err("model and handle must be nonempty".into());
        }
        match self.harnesses.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(harness);
            }
            Entry::Occupied(_) => return Err("duplicate Alloy model/handle".into()),
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
            tool_version: ran.then(|| ALLOY_VERSION.into()),
            scenarios,
        }
    }
    fn probe(&self, timeout: Duration) -> Result<(), String> {
        let actual = sha256::file(&self.jar)?;
        if actual != self.expected_sha256 {
            return Err("Alloy jar SHA-256 mismatch".into());
        }
        let output = execute(
            &self.java,
            &["-jar".into(), self.jar.as_os_str().into(), "version".into()],
            timeout,
        )?;
        if output.status == Some(0) && output.text().contains(ALLOY_VERSION) {
            Ok(())
        } else {
            Err(format!(
                "Alloy {ALLOY_VERSION} unavailable or version mismatch"
            ))
        }
    }
}
impl Backend for AlloyBackend {
    fn id(&self) -> BackendId {
        "alloy".to_owned().try_into().expect("valid id")
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
                    reason: "unregistered Alloy model/handle".into(),
                },
                false,
                vec![],
            );
        };
        if let Err(reason) = self.probe(deadline.saturating_duration_since(Instant::now())) {
            if Instant::now() >= deadline {
                return self.report(
                    plan,
                    Verdict::Incomplete {
                        reason: "Alloy deadline exceeded during integrity/version probe".into(),
                        explored: Bounds::new(),
                    },
                    false,
                    vec![],
                );
            }
            return self.report(plan, Verdict::Skipped { reason }, false, vec![]);
        }
        let Some(repeat) = exact_positive(&plan.bounds, "repeat") else {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "bounds must contain exactly positive repeat".into(),
                },
                false,
                vec![],
            );
        };
        if plan.bounds.len() != 1 || plan.seed.is_some() {
            return self.report(
                plan,
                Verdict::Error {
                    reason: "Alloy requires exactly repeat and no seed".into(),
                },
                false,
                vec![],
            );
        }
        let output_dir = match tempfile::tempdir() {
            Ok(v) => v,
            Err(e) => {
                return self.report(
                    plan,
                    Verdict::Error {
                        reason: format!("create Alloy output directory: {e}"),
                    },
                    false,
                    vec![],
                );
            }
        };
        let args = vec![
            "-jar".into(),
            self.jar.as_os_str().into(),
            "exec".into(),
            "-q".into(),
            "-r".into(),
            repeat.to_string().into(),
            "-c".into(),
            harness.command.clone().into(),
            "-o".into(),
            output_dir.path().as_os_str().into(),
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
                    reason: "Alloy deadline exceeded".into(),
                    explored: plan.bounds.clone(),
                },
                true,
                vec![],
            ),
            Ok(output) if output.limited => self.report(
                plan,
                Verdict::Incomplete {
                    reason: format!("Alloy output exceeded {MAX_OUTPUT} bytes"),
                    explored: plan.bounds.clone(),
                },
                true,
                vec![],
            ),
            Ok(output) => {
                if output.status != Some(0) {
                    self.report(
                        plan,
                        Verdict::Error {
                            reason: "Alloy execution failed".into(),
                        },
                        true,
                        vec![],
                    )
                } else {
                    match receipt_result(output_dir.path(), &harness.command) {
                        Ok(true) => self.report(
                            plan,
                            Verdict::Falsified {
                                reason: "Alloy produced a satisfying counterexample; its native instance has no project-specific scenario projector".into(),
                            },
                            true,
                            vec![],
                        ),
                        Ok(false) => self.report(
                            plan,
                            Verdict::Bounded {
                                bounds: plan.bounds.clone(),
                            },
                            true,
                            vec![],
                        ),
                        Err(reason) => self.report(plan, Verdict::Error { reason }, true, vec![]),
                    }
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
    let stderr = file.try_clone().map_err(|e| e.to_string())?;
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

fn receipt_result(directory: &std::path::Path, command: &str) -> Result<bool, String> {
    let path = directory.join("receipt.json");
    let before = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("inspect Alloy receipt: {error}"))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err("Alloy receipt is not a regular file".into());
    }
    let file = open_receipt(&path).map_err(|error| format!("read Alloy receipt: {error}"))?;
    let after = file
        .metadata()
        .map_err(|error| format!("inspect open Alloy receipt: {error}"))?;
    if !after.is_file() || before.len() != after.len() || before.len() > MAX_OUTPUT as u64 {
        return Err("Alloy receipt identity or size changed".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Alloy receipt: {error}"))?;
    if bytes.len() > MAX_OUTPUT {
        return Err("Alloy receipt exceeded output limit".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("parse Alloy receipt: {error}"))?;
    let commands = value
        .get("commands")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Alloy receipt lacks commands object".to_owned())?;
    if commands.len() != 1 || !commands.contains_key(command) {
        return Err("Alloy receipt command cardinality or identity mismatch".into());
    }
    let result = commands[command]
        .as_object()
        .ok_or_else(|| "Alloy receipt command result must be an object".to_owned())?;
    if result.get("name").and_then(serde_json::Value::as_str) != Some(command)
        || !matches!(
            result.get("type").and_then(serde_json::Value::as_str),
            Some("check" | "run")
        )
        || result
            .get("source")
            .and_then(serde_json::Value::as_str)
            .is_none()
        || result
            .get("overall")
            .and_then(serde_json::Value::as_u64)
            .is_none()
    {
        return Err("Alloy receipt command metadata is incomplete or inconsistent".into());
    }
    match result.get("solution") {
        None => Ok(false),
        Some(value) => value
            .as_array()
            .map(|solutions| !solutions.is_empty())
            .ok_or_else(|| "Alloy receipt solution must be an array".into()),
    }
}

fn open_receipt(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(0x20_000);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    options.custom_flags(0x100);
    options.open(path)
}
