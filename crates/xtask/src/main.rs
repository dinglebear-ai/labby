use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const DEFAULT_CASES: &[&str] = &["cold_return", "warm_return", "tool_fanout", "large_result"];

struct BenchCase {
    name: &'static str,
    code: &'static str,
    starts_per_process: usize,
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("codemode-bench") => {
            let opts = BenchOptions::parse(args.collect());
            if let Err(err) = run_bench(&opts) {
                eprintln!("codemode-bench failed: {err}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!(
                "usage: cargo run -p xtask -- codemode-bench --binary <path> [--label <name>] [--iterations N] [--warmup N] [--case NAME] [--json]"
            );
            std::process::exit(2);
        }
    }
}

struct BenchOptions {
    binary: PathBuf,
    label: String,
    iterations: usize,
    warmup: usize,
    timeout: Duration,
    cases: Vec<String>,
    json: bool,
}

impl BenchOptions {
    fn parse(args: Vec<String>) -> Self {
        let mut binary = None;
        let mut label = None;
        let mut iterations = 30usize;
        let mut warmup = 5usize;
        let mut timeout = Duration::from_secs(10);
        let mut cases = Vec::new();
        let mut json = false;

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--binary" => binary = iter.next().map(PathBuf::from),
                "--label" => label = iter.next(),
                "--iterations" => {
                    iterations = parse_usize("--iterations", iter.next());
                }
                "--warmup" => {
                    warmup = parse_usize("--warmup", iter.next());
                }
                "--timeout-ms" => {
                    timeout =
                        Duration::from_millis(parse_usize("--timeout-ms", iter.next()) as u64);
                }
                "--case" => {
                    cases.push(
                        iter.next()
                            .unwrap_or_else(|| die("--case requires a value")),
                    );
                }
                "--json" => json = true,
                "--help" | "-h" => {
                    println!(
                        "usage: cargo run -p xtask -- codemode-bench --binary <path> [--label <name>] [--iterations N] [--warmup N] [--case NAME] [--json]"
                    );
                    std::process::exit(0);
                }
                _ => die(format!("unknown argument: {arg}")),
            }
        }

        let binary = binary.unwrap_or_else(|| die("--binary is required"));
        let label = label.unwrap_or_else(|| {
            binary
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("labby")
                .to_string()
        });
        if cases.is_empty() {
            cases = DEFAULT_CASES.iter().map(ToString::to_string).collect();
        }

        Self {
            binary,
            label,
            iterations,
            warmup,
            timeout,
            cases,
            json,
        }
    }
}

fn parse_usize(flag: &str, value: Option<String>) -> usize {
    value
        .unwrap_or_else(|| die(format!("{flag} requires a value")))
        .parse()
        .unwrap_or_else(|err| die(format!("{flag} must be a positive integer: {err}")))
}

fn die<T>(message: impl AsRef<str>) -> T {
    eprintln!("{}", message.as_ref());
    std::process::exit(2);
}

fn bench_case(name: &str) -> BenchCase {
    match name {
        "cold_return" => BenchCase {
            name: "cold_return",
            code: "async () => 42",
            starts_per_process: 1,
        },
        "warm_return" => BenchCase {
            name: "warm_return",
            code: "async () => 42",
            starts_per_process: 10,
        },
        "tool_fanout" => BenchCase {
            name: "tool_fanout",
            code: r#"async () => {
  const calls = [];
  for (let i = 0; i < 20; i++) {
    calls.push(callTool("bench::echo", { i, text: "hello" }));
  }
  const out = await Promise.all(calls);
  return out.length;
}"#,
            starts_per_process: 5,
        },
        "large_result" => BenchCase {
            name: "large_result",
            code: r#"async () => {
  const out = [];
  for (let i = 0; i < 1000; i++) {
    out.push({ i, text: "abcdefghij" });
  }
  return out;
}"#,
            starts_per_process: 5,
        },
        _ => die(format!("unknown case: {name}")),
    }
}

fn run_bench(opts: &BenchOptions) -> Result<(), String> {
    if !opts.binary.exists() {
        return Err(format!("binary does not exist: {}", opts.binary.display()));
    }

    let mut rows = Vec::new();
    for case_name in &opts.cases {
        let case = bench_case(case_name);
        if opts.warmup > 0 {
            run_case(opts, &case, opts.warmup)?;
        }
        let samples = run_case(opts, &case, opts.iterations)?;
        let row = summarize(&opts.label, &opts.binary, case.name, &samples);
        if !opts.json {
            println!(
                "{:<18} {:<12} median={:.3}ms p95={:.3}ms mean={:.3}ms n={}",
                row.label, row.case, row.median_ms, row.p95_ms, row.mean_ms, row.samples
            );
        }
        rows.push(row);
    }

    if opts.json {
        let json_rows: Vec<Value> = rows.iter().map(BenchRow::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "results": json_rows })).unwrap()
        );
    }

    Ok(())
}

fn run_case(opts: &BenchOptions, case: &BenchCase, iterations: usize) -> Result<Vec<f64>, String> {
    let mut samples = Vec::with_capacity(iterations);
    if case.starts_per_process <= 1 {
        for _ in 0..iterations {
            let mut runner = RunnerProcess::spawn(&opts.binary)?;
            let sample = runner.run_start(case, opts.timeout);
            runner.terminate();
            samples.push(sample?);
        }
        return Ok(samples);
    }

    let mut remaining = iterations;
    while remaining > 0 {
        let mut runner = RunnerProcess::spawn(&opts.binary)?;
        let batch = remaining.min(case.starts_per_process);
        for _ in 0..batch {
            samples.push(runner.run_start(case, opts.timeout)?);
        }
        runner.terminate();
        remaining -= batch;
    }
    Ok(samples)
}

struct RunnerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RunnerProcess {
    fn spawn(binary: &PathBuf) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .args(["internal", "code-mode-runner"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("spawn {}: {err}", binary.display()))?;
        let stdin = child.stdin.take().ok_or("runner stdin missing")?;
        let stdout = child.stdout.take().ok_or("runner stdout missing")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn run_start(&mut self, case: &BenchCase, timeout: Duration) -> Result<f64, String> {
        let started = Instant::now();
        self.write_frame(&json!({
            "type": "start",
            "code": case.code,
            "proxy": "",
            "timeout_ms": timeout.as_millis(),
        }))?;
        loop {
            let frame = self.read_frame(timeout)?;
            match frame.get("type").and_then(Value::as_str) {
                Some("done") => return Ok(started.elapsed().as_secs_f64() * 1000.0),
                Some("error") => return Err(format!("runner error: {frame}")),
                Some("tool_call") => {
                    self.write_frame(
                        &json!({
                            "type": "tool_result",
                            "seq": frame["seq"],
                            "result": { "ok": true, "echo": frame.get("params").cloned().unwrap_or(Value::Null) }
                        }),
                    )?;
                }
                Some("artifact_write") => {
                    self.write_frame(
                        &json!({
                            "type": "tool_result",
                            "seq": frame["seq"],
                            "result": { "ok": true, "path": frame.get("path").cloned().unwrap_or(Value::Null) }
                        }),
                    )?;
                }
                Some("snippet_resolve") => {
                    self.write_frame(&json!({
                        "type": "snippet_resolved",
                        "seq": frame["seq"],
                        "code": "async () => null",
                        "input": frame.get("input").cloned().unwrap_or_else(|| json!({}))
                    }))?;
                }
                other => return Err(format!("unexpected runner frame type {other:?}: {frame}")),
            }
        }
    }

    fn write_frame(&mut self, frame: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, frame).map_err(|err| err.to_string())?;
        self.stdin.write_all(b"\n").map_err(|err| err.to_string())?;
        self.stdin.flush().map_err(|err| err.to_string())
    }

    fn read_frame(&mut self, _timeout: Duration) -> Result<Value, String> {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|err| format!("read runner output: {err}"))?;
        if line.is_empty() {
            let exit = self.child.try_wait().map_err(|err| err.to_string())?;
            return Err(format!("runner closed stdout; exit={exit:?}"));
        }
        serde_json::from_str(&line)
            .map_err(|err| format!("runner emitted invalid JSON: {err}: {line}"))
    }

    fn terminate(mut self) {
        drop(self.stdin);
        match self.child.wait_timeout(Duration::from_secs(5)) {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                drop(self.child.kill());
                drop(self.child.wait());
            }
        }
    }
}

trait WaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeout for Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if started.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

struct BenchRow {
    label: String,
    binary: String,
    case: String,
    samples: usize,
    min_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    mean_ms: f64,
}

impl BenchRow {
    fn to_json(&self) -> Value {
        json!({
            "label": self.label,
            "binary": self.binary,
            "case": self.case,
            "samples": self.samples,
            "min_ms": self.min_ms,
            "median_ms": self.median_ms,
            "p95_ms": self.p95_ms,
            "max_ms": self.max_ms,
            "mean_ms": self.mean_ms,
        })
    }
}

fn summarize(label: &str, binary: &PathBuf, case: &str, samples: &[f64]) -> BenchRow {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let sum: f64 = sorted.iter().sum();
    BenchRow {
        label: label.to_string(),
        binary: binary.display().to_string(),
        case: case.to_string(),
        samples: sorted.len(),
        min_ms: *sorted.first().unwrap_or(&f64::NAN),
        median_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        max_ms: *sorted.last().unwrap_or(&f64::NAN),
        mean_ms: sum / sorted.len() as f64,
    }
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * pct).round() as usize;
    sorted[idx]
}
