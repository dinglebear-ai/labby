use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "labby.proxy-proof";
const VERSION: u64 = 1;
const DEFAULT_OUTPUT: &str = "target/proxy-proof";
const SECRET_CANARIES: &[&str] = &[
    "proxy-verifier-bearer-canary",
    "proxy-verifier-lease-canary",
    "proxy-verifier-user-canary",
];

const HELP: &str = "\
Verify Labby's stdio MCP proxy with the real Labby binary and fixture server.\n\n\
Usage: cargo run -p xtask -- proxy-verify [OPTIONS]\n\n\
Options:\n\
  --binary PATH   Labby binary (default: target/debug/labby)\n\
  --output DIR    Sanitized proof directory (default: target/proxy-proof)\n\
  --keep-temp     Keep the private verifier scratch directory\n\
  --json          Print the final result as JSON\n\
  -h, --help      Print help\n\n\
Set LABBY_PROXY_LIVE=1 to enable the real Tailscale HTTPS and cleanup gate.\n";

#[derive(Debug, Clone)]
pub(crate) struct ProxyVerifyOptions {
    binary: PathBuf,
    output: PathBuf,
    keep_temp: bool,
    json: bool,
    live: bool,
}

impl ProxyVerifyOptions {
    pub(crate) fn parse(args: Vec<String>) -> Result<Option<Self>, String> {
        let mut binary = None;
        let mut output = None;
        let mut keep_temp = false;
        let mut json = false;
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--binary" => binary = Some(required_path("--binary", iter.next())?),
                "--output" => output = Some(required_path("--output", iter.next())?),
                "--keep-temp" => keep_temp = true,
                "--json" => json = true,
                "--help" | "-h" => {
                    print!("{HELP}");
                    return Ok(None);
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        Ok(Some(Self {
            binary: binary.unwrap_or_else(|| target.join("debug/labby")),
            output: output.unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT)),
            keep_temp,
            json,
            live: std::env::var("LABBY_PROXY_LIVE").as_deref() == Ok("1"),
        }))
    }
}

fn required_path(flag: &str, value: Option<String>) -> Result<PathBuf, String> {
    value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[derive(Debug, Clone)]
struct Scenario {
    name: &'static str,
    command: &'static str,
    status: &'static str,
    exit_status: i32,
    evidence: Vec<&'static str>,
}

impl Scenario {
    fn passed(name: &'static str, command: &'static str, evidence: Vec<&'static str>) -> Self {
        Self {
            name,
            command,
            status: "passed",
            exit_status: 0,
            evidence,
        }
    }

    fn failed(name: &'static str, command: &'static str, evidence: &'static str) -> Self {
        Self {
            name,
            command,
            status: "failed",
            exit_status: 1,
            evidence: vec![evidence],
        }
    }

    fn skipped(name: &'static str, command: &'static str, evidence: &'static str) -> Self {
        Self {
            name,
            command,
            status: "skipped",
            exit_status: 0,
            evidence: vec![evidence],
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "command": self.command,
            "status": self.status,
            "exit_status": self.exit_status,
            "evidence": self.evidence,
        })
    }
}

pub(crate) fn run(options: &ProxyVerifyOptions) -> Result<bool, String> {
    prepare_output(&options.output)?;
    let scratch = Scratch::new(options.keep_temp)?;
    let repo = repo_root()?;
    let fixture = options
        .binary
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(binary_name("stdio-mcp-fixture"));
    let mut scenarios = Vec::new();

    if !options.binary.is_file() {
        scenarios.push(Scenario::failed(
            "binary_resolution",
            "labby binary validation",
            "configured Labby binary was not found",
        ));
        return finish(options, &repo, &scenarios);
    }
    if !fixture.is_file() {
        let status = Command::new("cargo")
            .args([
                "build",
                "-p",
                "labby",
                "--all-features",
                "--features",
                "proxy-testkit",
                "--bins",
                "--locked",
            ])
            .current_dir(&repo)
            .status()
            .map_err(|err| format!("build proxy fixture: {err}"))?;
        if !status.success() || !fixture.is_file() {
            scenarios.push(Scenario::failed(
                "fixture_build",
                "cargo build proxy fixture",
                "fixture build failed",
            ));
            return finish(options, &repo, &scenarios);
        }
    }

    scenarios.push(run_direct_probe(
        "local_no_auth_random_port",
        &options.binary,
        &fixture,
        scratch.path(),
        DirectAuth::None,
    ));
    scenarios.push(run_direct_probe(
        "local_bearer_fixed_port",
        &options.binary,
        &fixture,
        scratch.path(),
        DirectAuth::Bearer,
    ));

    for (name, command, args, evidence) in test_scenarios() {
        scenarios.push(run_command_scenario(&repo, name, command, args, evidence));
    }

    scenarios.push(if options.live {
        run_live_tailscale(&options.binary, &fixture, scratch.path())
    } else {
        Scenario::skipped(
            "live_tailscale_https_cleanup",
            "LABBY_PROXY_LIVE=1 proxy live gate",
            "opt-in live gate disabled",
        )
    });

    finish(options, &repo, &scenarios)
}

fn prepare_output(output: &Path) -> Result<(), String> {
    fs::create_dir_all(output).map_err(|err| format!("create {}: {err}", output.display()))?;
    for name in [
        "manifest.json",
        "commands.jsonl",
        "summary.md",
        "redaction-scan.json",
    ] {
        let path = output.join(name);
        if path.is_file() {
            fs::remove_file(&path).map_err(|err| format!("replace {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|err| format!("resolve repository root: {err}"))?;
    if !output.status.success() {
        return Err("git rev-parse --show-toplevel failed".to_string());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

enum DirectAuth {
    None,
    Bearer,
}

fn run_direct_probe(
    name: &'static str,
    binary: &Path,
    fixture: &Path,
    scratch: &Path,
    auth: DirectAuth,
) -> Scenario {
    match direct_probe(binary, fixture, scratch, auth) {
        Ok(evidence) => Scenario::passed(name, "real Labby direct proxy probe", evidence),
        Err(error) => {
            eprintln!(
                "proxy-verify: {name} failed: {}",
                sanitize_diagnostic(&error)
            );
            Scenario::failed(
                name,
                "real Labby direct proxy probe",
                "direct proxy probe failed",
            )
        }
    }
}

fn sanitize_diagnostic(value: &str) -> String {
    let mut sanitized = value.to_string();
    for canary in SECRET_CANARIES {
        sanitized = sanitized.replace(canary, "[REDACTED]");
    }
    if let Some(home) = std::env::var_os("HOME") {
        sanitized = sanitized.replace(home.to_string_lossy().as_ref(), "[HOME]");
    }
    sanitized = sanitized.replace(std::env::temp_dir().to_string_lossy().as_ref(), "[TEMP]");
    sanitized
}

fn direct_probe(
    binary: &Path,
    fixture: &Path,
    scratch: &Path,
    auth: DirectAuth,
) -> Result<Vec<&'static str>, String> {
    let home = scratch.join(match auth {
        DirectAuth::None => "local-none",
        DirectAuth::Bearer => "local-bearer",
    });
    fs::create_dir_all(&home).map_err(|err| err.to_string())?;
    let state_home = home.join("state");
    fs::create_dir_all(&state_home).map_err(|err| err.to_string())?;
    let pid_file = home.join("child.pid");
    let mut command = Command::new(binary);
    command.args(["--json", "proxy", "--local"]);
    let bearer = matches!(auth, DirectAuth::Bearer);
    if bearer {
        let port = reserve_port()?;
        command
            .args([
                "--auth",
                "bearer",
                "--bearer-token",
                SECRET_CANARIES[0],
                "--port",
            ])
            .arg(port.to_string());
    } else {
        command.args(["--auth", "none"]);
    }
    command
        .arg(fixture)
        .arg("--pid-file")
        .arg(&pid_file)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &state_home)
        .env("LABBY_HOME", &home)
        .env("PROXY_SCRUB_CANARY", "must-not-be-inherited")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("spawn proxy: {err}"))?;
    let stdout = child.stdout.take().ok_or("proxy stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("proxy stderr unavailable")?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let line = BufReader::new(stdout).lines().next().transpose();
        drop(sender.send(line));
    });
    let line = match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => {
            let mut diagnostic = String::new();
            drop(stderr.read_to_string(&mut diagnostic));
            return Err(format!(
                "proxy exited before readiness: {}",
                sanitize_diagnostic(diagnostic.trim())
            ));
        }
        Ok(Err(error)) => return Err(format!("read readiness: {error}")),
        Err(_) => {
            stop_proxy(&mut child);
            let mut diagnostic = String::new();
            drop(stderr.read_to_string(&mut diagnostic));
            return Err(format!(
                "proxy readiness timed out: {}",
                sanitize_diagnostic(diagnostic.trim())
            ));
        }
    };
    let ready: Value =
        serde_json::from_str(&line).map_err(|err| format!("readiness JSON: {err}"))?;
    let url = ready["url"].as_str().ok_or("readiness URL missing")?;
    let local_addr = ready["local_addr"]
        .as_str()
        .ok_or("local address missing")?;
    if !local_addr.starts_with("127.0.0.1:") {
        stop_proxy(&mut child);
        return Err("proxy did not bind loopback".to_string());
    }

    let token = bearer.then_some(SECRET_CANARIES[0]);
    if bearer {
        let status = curl_status(url, None, "tools/list", None)?;
        if status != 401 {
            stop_proxy(&mut child);
            return Err("bearer challenge missing".to_string());
        }
    }
    for (method, marker) in [
        ("tools/list", "fixture.echo"),
        ("resources/list", "fixture://status"),
        ("prompts/list", "fixture.prompt"),
    ] {
        let (status, body) = curl_json(url, token, method, None)?;
        if status != 200 || !body.contains(marker) {
            stop_proxy(&mut child);
            return Err(format!("{method} passthrough failed"));
        }
    }
    for header in [
        ("Host", "attacker.invalid"),
        ("Origin", "https://attacker.invalid"),
    ] {
        if curl_status(url, token, "tools/list", Some(header))? != 403 {
            stop_proxy(&mut child);
            return Err("hostile request was not rejected".to_string());
        }
    }
    stop_proxy(&mut child);
    let mut stderr_text = String::new();
    stderr
        .read_to_string(&mut stderr_text)
        .map_err(|err| format!("read proxy stderr: {err}"))?;
    if SECRET_CANARIES
        .iter()
        .any(|canary| stderr_text.contains(canary))
    {
        return Err("secret appeared in proxy diagnostics".to_string());
    }
    if process_from_pid_file_alive(&pid_file) {
        return Err("fixture child residue detected".to_string());
    }
    Ok(if bearer {
        vec![
            "fixed loopback port",
            "bearer challenge and acceptance",
            "tools resources prompts passthrough",
            "hostile Host and Origin rejected",
            "child cleanup confirmed",
        ]
    } else {
        vec![
            "random loopback port",
            "no-auth local proxy",
            "tools resources prompts passthrough",
            "hostile Host and Origin rejected",
            "child cleanup confirmed",
        ]
    })
}

fn reserve_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|err| err.to_string())
}

fn request_body(method: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name":"proxy-verifier","version":"1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
    .to_string()
}

fn curl_json(
    url: &str,
    token: Option<&str>,
    method: &str,
    extra_header: Option<(&str, &str)>,
) -> Result<(u16, String), String> {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--output",
        "-",
        "--write-out",
        "\n%{http_code}",
        "--header",
        "Content-Type: application/json",
        "--header",
        "Accept: application/json, text/event-stream",
        "--header",
        "MCP-Protocol-Version: 2026-07-28",
        "--data",
        &request_body(method),
    ]);
    command.args(["--header", &format!("Mcp-Method: {method}")]);
    if let Some(token) = token {
        command.args(["--header", &format!("Authorization: Bearer {token}")]);
    }
    if let Some((name, value)) = extra_header {
        command.args(["--header", &format!("{name}: {value}")]);
    }
    let output = command
        .arg(url)
        .output()
        .map_err(|err| format!("run curl: {err}"))?;
    if !output.status.success() {
        return Err("curl request failed".to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').ok_or("curl status missing")?;
    let status = status.parse::<u16>().map_err(|err| err.to_string())?;
    Ok((status, body.to_string()))
}

fn curl_status(
    url: &str,
    token: Option<&str>,
    method: &str,
    extra_header: Option<(&str, &str)>,
) -> Result<u16, String> {
    curl_json(url, token, method, extra_header).map(|(status, _)| status)
}

fn stop_proxy(child: &mut Child) {
    #[cfg(unix)]
    drop(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status(),
    );
    #[cfg(not(unix))]
    drop(child.kill());
    if wait_timeout(child, Duration::from_secs(8))
        .ok()
        .flatten()
        .is_none()
    {
        drop(child.kill());
        drop(child.wait());
    }
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() >= timeout {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn process_from_pid_file_alive(pid_file: &Path) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = fs::read_to_string(pid_file) else {
            return false;
        };
        Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid_file;
        false
    }
}

type TestScenario = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

fn test_scenarios() -> Vec<TestScenario> {
    vec![
        (
            "command_resolution_and_validation",
            "cargo test proxy command resolution",
            &[
                "test",
                "-p",
                "labby",
                "--all-features",
                "--features",
                "proxy-testkit",
                "--locked",
                "proxy::command::tests::",
            ],
            &["executable shebang JavaScript Python PATH and unknown-extension resolution"],
        ),
        (
            "proxy_runtime_and_oauth",
            "cargo test stdio proxy runtime",
            &[
                "test",
                "-p",
                "labby",
                "--all-features",
                "--features",
                "proxy-testkit",
                "--locked",
                "--test",
                "stdio_proxy_runtime",
                "--",
                "--test-threads=1",
            ],
            &[
                "discovery bind bearer auth state router child HTTP and Ctrl+C lifecycle",
                "exact OAuth metadata challenge audience and scope",
                "lease create renew release collision renewal-failure cleanup",
                "environment scrubbing inheritance and non-UTF-8 arguments",
                "modern legacy and Auto lifecycle passthrough",
            ],
        ),
        (
            "fake_tailscale_lifecycle",
            "cargo test fake Tailscale lifecycle",
            &[
                "test",
                "-p",
                "labby",
                "--all-features",
                "--features",
                "proxy-testkit",
                "--locked",
                "--test",
                "tailscale_serve",
                "--",
                "--test-threads=1",
            ],
            &[
                "plan claim readiness collision and Serve exit",
                "exact ownership cleanup preserves unrelated mappings",
            ],
        ),
        (
            "proxy_runtime_unit_faults",
            "cargo test proxy runtime unit faults",
            &[
                "test",
                "-p",
                "labby",
                "--all-features",
                "--features",
                "proxy-testkit",
                "--locked",
                "proxy::runtime_tests::",
            ],
            &["configuration and auth policy validation fault cleanup"],
        ),
    ]
}

fn run_command_scenario(
    repo: &Path,
    name: &'static str,
    command_name: &'static str,
    args: &'static [&'static str],
    evidence: &'static [&'static str],
) -> Scenario {
    match Command::new("cargo").args(args).current_dir(repo).status() {
        Ok(status) if status.success() => Scenario::passed(name, command_name, evidence.to_vec()),
        _ => Scenario::failed(name, command_name, "verification command failed"),
    }
}

fn run_live_tailscale(binary: &Path, fixture: &Path, scratch: &Path) -> Scenario {
    let home = scratch.join("live-tailscale");
    let state_home = home.join("state");
    if fs::create_dir_all(&state_home).is_err() {
        return Scenario::failed(
            "live_tailscale_https_cleanup",
            "real Tailscale HTTPS gate",
            "live scratch setup failed",
        );
    }
    let before = tailscale_status();
    let mut child = match Command::new(binary)
        .args(["--json", "proxy", "--auth", "tailnet"])
        .arg(fixture)
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &state_home)
        .env("LABBY_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return Scenario::failed(
                "live_tailscale_https_cleanup",
                "real Tailscale HTTPS gate",
                "live proxy spawn failed",
            );
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let line = BufReader::new(stdout).lines().next().transpose();
    let passed = line
        .ok()
        .flatten()
        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        .and_then(|ready| ready["url"].as_str().map(str::to_owned))
        .is_some_and(|url| curl_status(&url, None, "tools/list", None).ok() == Some(200));
    stop_proxy(&mut child);
    let after = tailscale_status();
    if passed && before == after {
        Scenario::passed(
            "live_tailscale_https_cleanup",
            "real Tailscale HTTPS gate",
            vec![
                "HTTPS discovery succeeded",
                "Ctrl+C restored normalized Serve state",
            ],
        )
    } else {
        Scenario::failed(
            "live_tailscale_https_cleanup",
            "real Tailscale HTTPS gate",
            "live HTTPS or cleanup verification failed",
        )
    }
}

fn tailscale_status() -> Option<Value> {
    let output = Command::new("tailscale")
        .args(["serve", "status", "--json"])
        .output()
        .ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

fn finish(
    options: &ProxyVerifyOptions,
    repo: &Path,
    scenarios: &[Scenario],
) -> Result<bool, String> {
    let passed = scenarios.iter().all(|scenario| scenario.status != "failed");
    let commands_path = options.output.join("commands.jsonl");
    let mut commands = File::create(&commands_path).map_err(|err| err.to_string())?;
    for scenario in scenarios {
        serde_json::to_writer(&mut commands, &scenario.to_json()).map_err(|err| err.to_string())?;
        commands.write_all(b"\n").map_err(|err| err.to_string())?;
    }
    commands.flush().map_err(|err| err.to_string())?;

    let summary = format!(
        "# Labby proxy verification\n\nResult: {}\n\nScenarios: {} passed, {} skipped, {} failed.\n",
        if passed { "passed" } else { "failed" },
        scenarios
            .iter()
            .filter(|scenario| scenario.status == "passed")
            .count(),
        scenarios
            .iter()
            .filter(|scenario| scenario.status == "skipped")
            .count(),
        scenarios
            .iter()
            .filter(|scenario| scenario.status == "failed")
            .count(),
    );
    fs::write(options.output.join("summary.md"), summary).map_err(|err| err.to_string())?;

    let checks = scan_proof(&options.output)?;
    let redaction = json!({
        "status": "passed",
        "checks": checks,
    });
    write_json(&options.output.join("redaction-scan.json"), &redaction)?;
    let artifacts = ["commands.jsonl", "redaction-scan.json", "summary.md"]
        .into_iter()
        .map(|name| Ok((name, sha256_file(&options.output.join(name))?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let manifest = json!({
        "schema": SCHEMA,
        "version": VERSION,
        "source": {
            "commit": git_value(repo, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string()),
            "tree": if git_clean(repo) { "clean" } else { "modified" },
        },
        "live_tailscale": if options.live { "enabled" } else { "skipped" },
        "scenarios": scenarios.iter().map(Scenario::to_json).collect::<Vec<_>>(),
        "artifacts": artifacts,
        "result": if passed { "passed" } else { "failed" },
    });
    write_json(&options.output.join("manifest.json"), &manifest)?;
    scan_proof(&options.output)?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string(
                &json!({"result": manifest["result"], "manifest": "manifest.json"})
            )
            .unwrap()
        );
    } else {
        println!(
            "proxy verification {}: {}",
            manifest["result"].as_str().unwrap(),
            options.output.display()
        );
    }
    Ok(passed)
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|err| err.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|err| format!("write {}: {err}", path.display()))
}

fn git_value(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_clean(repo: &Path) -> bool {
    git_value(repo, &["status", "--porcelain"]).is_some_and(|value| value.is_empty())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn scan_proof(output: &Path) -> Result<Vec<&'static str>, String> {
    let mut forbidden_values = SECRET_CANARIES
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME") {
        forbidden_values.push(home.to_string_lossy().into_owned());
    }
    if let Ok(user) = std::env::var("USER")
        && user.len() >= 3
    {
        forbidden_values.push(user);
    }
    forbidden_values.push(std::env::temp_dir().to_string_lossy().into_owned());
    forbidden_values.push(format!("labby-proxy-verify-{}", std::process::id()));
    forbidden_values.push(format!(
        "{}{}{}",
        char::from(34),
        std::process::id(),
        char::from(34)
    ));

    for entry in fs::read_dir(output).map_err(|err| err.to_string())? {
        let path = entry.map_err(|err| err.to_string())?.path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).map_err(|err| err.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        if let Some(forbidden) = forbidden_values
            .iter()
            .filter(|value| !value.is_empty())
            .find(|value| text.contains(value.as_str()))
        {
            return Err(format!(
                "dynamic or sensitive value was found in proof artifact {} ({})",
                path.display(),
                redacted_value_class(forbidden)
            ));
        }
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|err| format!("parse proof JSON {}: {err}", path.display()))?;
            scan_json_value(&value, &path)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let value: Value = serde_json::from_str(line)
                    .map_err(|err| format!("parse proof JSONL {}: {err}", path.display()))?;
                scan_json_value(&value, &path)?;
            }
        }
    }
    Ok(vec![
        "secret canaries",
        "opaque lease identifier shapes",
        "user and home paths",
        "temporary scratch paths",
        "process identifiers",
        "timestamp and port structured fields",
    ])
}

fn redacted_value_class(value: &str) -> &'static str {
    if SECRET_CANARIES.contains(&value) {
        "secret canary"
    } else if value.contains("labby-proxy-verify-")
        || value == std::env::temp_dir().to_string_lossy()
    {
        "temporary path"
    } else if value.starts_with('"') {
        "process identifier"
    } else {
        "user or home path"
    }
}

fn scan_json_value(value: &Value, path: &Path) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if [
                    "token",
                    "secret",
                    "lease_id",
                    "leaseid",
                    "pid",
                    "port",
                    "timestamp",
                    "temp_dir",
                    "home",
                    "username",
                ]
                .iter()
                .any(|sensitive| {
                    normalized == *sensitive || normalized.ends_with(&format!("_{sensitive}"))
                }) {
                    return Err(format!(
                        "sensitive structured field {key:?} found in proof artifact {}",
                        path.display()
                    ));
                }
                scan_json_value(value, path)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                scan_json_value(value, path)?;
            }
        }
        Value::String(value) if looks_like_opaque_lease_id(value) => {
            return Err(format!(
                "opaque lease identifier shape found in proof artifact {}",
                path.display()
            ));
        }
        _ => {}
    }
    Ok(())
}

fn looks_like_opaque_lease_id(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

struct Scratch {
    path: PathBuf,
    keep: bool,
}

impl Scratch {
    fn new(keep: bool) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("labby-proxy-verify-{}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|err| err.to_string())?;
        }
        fs::create_dir(&path).map_err(|err| err.to_string())?;
        Ok(Self { path, keep })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.keep {
            drop(fs::remove_dir_all(&self.path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_uses_deterministic_defaults_and_live_is_opt_in() {
        let options = ProxyVerifyOptions::parse(Vec::new()).unwrap().unwrap();
        assert_eq!(options.output, PathBuf::from(DEFAULT_OUTPUT));
        assert!(!options.keep_temp);
        assert!(!options.json);
    }

    #[test]
    fn scenario_names_are_normalized_literals() {
        for (name, ..) in test_scenarios() {
            assert!(name.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'));
        }
    }

    #[test]
    fn successful_proof_manifests_are_byte_stable_across_output_roots() {
        let repo = repo_root().unwrap();
        let root = std::env::temp_dir().join(format!(
            "labby-proxy-proof-success-determinism-{}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&root));
        fs::create_dir(&root).unwrap();
        let scenarios = vec![
            Scenario::passed(
                "deterministic_success",
                "synthetic successful verifier scenario",
                vec!["normalized evidence"],
            ),
            Scenario::skipped(
                "live_tailscale_https_cleanup",
                "LABBY_PROXY_LIVE=1 proxy live gate",
                "opt-in live gate disabled",
            ),
        ];
        let mut manifests = Vec::new();
        for suffix in ["a", "b"] {
            let output = root.join(suffix);
            let options = ProxyVerifyOptions {
                binary: PathBuf::from("target/debug/labby"),
                output: output.clone(),
                keep_temp: false,
                json: false,
                live: false,
            };
            prepare_output(&output).unwrap();
            assert!(finish(&options, &repo, &scenarios).unwrap());
            manifests.push(fs::read(output.join("manifest.json")).unwrap());
        }
        assert_eq!(manifests[0], manifests[1]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_scan_rejects_sensitive_structured_fields_and_lease_shapes() {
        let root = std::env::temp_dir().join(format!(
            "labby-proxy-proof-scan-test-{}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&root));
        fs::create_dir(&root).unwrap();
        write_json(
            &root.join("manifest.json"),
            &json!({"lease_id": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}),
        )
        .unwrap();
        assert!(scan_proof(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
