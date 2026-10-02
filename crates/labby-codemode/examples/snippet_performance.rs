//! Reproducible, isolated listing + real QuickJS batch benchmark; JSON output.
#![allow(unknown_lints)]
#![allow(clippy::unused_async_trait_impl)]
use labby_codemode::error::ToolError;
use labby_codemode::host::{ExecCtx, ResolvedSnippet, ToolCallOutcome, ToolsRender};
use labby_codemode::snippet::store::list_snippets;
use labby_codemode::{
    CatalogDescriptor, CodeModeBroker, CodeModeCallError, CodeModeCaller, CodeModeCatalogKind,
    CodeModeConfig, CodeModeHost, CodeModeSurface, RunnerPool, RunnerSpawn, ToolScope,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
struct Host {
    pool: RunnerPool,
    admission: tokio::sync::Semaphore,
    latency: Duration,
    calls: AtomicUsize,
    active: AtomicUsize,
    maximum: AtomicUsize,
}
impl Host {
    fn new(runner: &Path, latency: u64, concurrency: usize) -> Self {
        Self {
            pool: RunnerPool::with_spawn(RunnerSpawn {
                program: runner.into(),
                args: vec!["internal".into(), "code-mode-runner".into()],
            }),
            admission: tokio::sync::Semaphore::new(concurrency),
            latency: Duration::from_millis(latency),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
        }
    }
}
impl CodeModeHost for Host {
    async fn list_tools(
        &self,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: bool,
        _: bool,
    ) -> Result<ToolsRender, ToolError> {
        let entries: Arc<[CatalogDescriptor]> = Arc::from([CatalogDescriptor::tool(
            "bench",
            "read",
            "Synthetic read with configured async latency",
            Some(json!({"type":"object","properties":{"i":{"type":"integer"}},"required":["i"]})),
            None,
        )]);
        let catalog_json: Arc<str> = Arc::from(serde_json::to_string(entries.as_ref()).unwrap());
        Ok(ToolsRender {
            fingerprint: "bench".into(),
            embedding_fingerprint: "bench".into(),
            serialized_size: catalog_json.len(),
            entries,
            catalog_json,
            withheld: Arc::from([]),
        })
    }
    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        if id != "bench::read" {
            return Err(CodeModeCallError::new(
                "unknown_tool",
                "benchmark tool missing",
            ));
        }
        let _permit = self
            .admission
            .acquire()
            .await
            .map_err(|_| CodeModeCallError::new("cancelled", "benchmark admission closed"))?;
        self.calls.fetch_add(1, Ordering::Relaxed);
        let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        self.maximum.fetch_max(active, Ordering::Relaxed);
        tokio::time::sleep(self.latency).await;
        self.active.fetch_sub(1, Ordering::Relaxed);
        Ok(ToolCallOutcome {
            value: json!({"i":params["i"]}),
            ui: None,
        })
    }
    async fn resolve_snippet(&self, _: &str, _: Value) -> Result<ResolvedSnippet, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: "benchmark has no nested snippets".into(),
        })
    }
    async fn semantic_rank(
        &self,
        _: String,
        _: usize,
        _: &[CodeModeCatalogKind],
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        Ok(vec![])
    }
    async fn config(&self) -> CodeModeConfig {
        CodeModeConfig {
            timeout_ms: 3000,
            ..CodeModeConfig::default()
        }
    }
    fn runner_pool(&self) -> &RunnerPool {
        &self.pool
    }
    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        labby_openapi::OpenApiRegistry::default()
    }
    fn openapi_http_client(&self) -> reqwest::Client {
        labby_openapi::http::build_dispatch_client().expect("build hardened benchmark client")
    }
}
fn number(
    args: &[String],
    flag: &str,
    default: usize,
    max: usize,
) -> Result<usize, Box<dyn Error>> {
    let value = if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.get(index + 1)
            .ok_or("option requires value")?
            .parse()?
    } else {
        default
    };
    if !(1..=max).contains(&value) {
        return Err(format!("{flag} must be 1..={max}").into());
    }
    Ok(value)
}
fn summary(samples: &[f64]) -> Value {
    let mut ordered = samples.to_vec();
    ordered.sort_by(f64::total_cmp);
    json!({"samples_ms":samples,"median_ms":f64::midpoint(ordered[(ordered.len()-1)/2],ordered[ordered.len()/2]),"p95_ms":ordered[(ordered.len()*95).div_ceil(100).saturating_sub(1)],"samples":samples.len()})
}
fn list_sample(home: &Path, builtin: &Path) -> Result<f64, Box<dyn Error>> {
    let start = Instant::now();
    let list = list_snippets(home, builtin)?;
    std::hint::black_box(list);
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}
async fn batch(host: &Host, code: &str, calls: usize) -> Result<Value, Box<dyn Error>> {
    let before = host.calls.load(Ordering::Relaxed);
    let start = Instant::now();
    let response = Box::pin(CodeModeBroker::new(Some(host)).execute(
        code,
        CodeModeCaller::TrustedLocal,
        CodeModeSurface::Cli,
        host.config().await,
        ToolScope::new(vec!["bench".into()], vec![]),
        None,
    ))
    .await?;
    if response.calls.len() != calls
        || host.calls.load(Ordering::Relaxed) - before != calls
        || response
            .result
            .as_ref()
            .and_then(|v| v.get("failed"))
            .and_then(Value::as_u64)
            != Some(0)
    {
        return Err("batch did not execute expected real host calls".into());
    }
    Ok(
        json!({"wall_ms":start.elapsed().as_secs_f64()*1000.0,"calls":response.calls.len(),"max_in_flight":host.maximum.swap(0,Ordering::Relaxed)}),
    )
}
async fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.first().is_some_and(|arg| arg == "--list-sample") {
        if args.len() != 3 {
            return Err("list-sample requires two paths".into());
        }
        println!("{}", list_sample(Path::new(&args[1]), Path::new(&args[2]))?);
        return Ok(());
    }
    if !args.len().is_multiple_of(2) {
        return Err("each option requires one value".into());
    }
    let mut flags = std::collections::HashSet::new();
    for option in args.chunks_exact(2) {
        if ![
            "--runner",
            "--samples",
            "--snippets",
            "--calls",
            "--concurrency",
            "--latency-ms",
        ]
        .contains(&option[0].as_str())
            || !flags.insert(&option[0])
        {
            return Err("unknown or repeated benchmark option".into());
        }
    }
    let index=args.iter().position(|arg|arg=="--runner").ok_or("usage: --runner /absolute/labby [--samples 10] [--snippets 32] [--calls 16] [--concurrency 8] [--latency-ms 2]")?;
    let runner = PathBuf::from(args.get(index + 1).ok_or("--runner requires path")?);
    if !runner.is_absolute() || !runner.is_file() {
        return Err("runner must be an existing absolute binary path".into());
    }
    let samples = number(args, "--samples", 10, 30)?;
    let snippets = number(args, "--snippets", 32, 128)?;
    let calls = number(args, "--calls", 16, 64)?;
    let concurrency = number(args, "--concurrency", 8, 32)?.min(calls);
    let latency = number(args, "--latency-ms", 2, 10)?;
    let home = tempfile::tempdir()?;
    let builtin = tempfile::tempdir()?;
    fs::create_dir(home.path().join("snippets"))?;
    for i in 0..snippets {
        fs::write(
            home.path().join(format!("snippets/bench-{i}.md")),
            format!(
                "---\nname: bench-{i}\ndescription: Benchmark\ntools: []\n---\n```js\nasync () => ({{ok:true}})\n```\n"
            ),
        )?;
    }
    let started = Instant::now();
    let mut cold = Vec::new();
    let mut warm = Vec::new();
    for _ in 0..samples {
        let output = Command::new(std::env::current_exe()?)
            .args(["--list-sample"])
            .arg(home.path())
            .arg(builtin.path())
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        cold.push(serde_json::from_slice::<f64>(&output.stdout)?);
    }
    list_sample(home.path(), builtin.path())?;
    for _ in 0..samples {
        warm.push(list_sample(home.path(), builtin.path())?);
    }
    let code = format!(
        "async () => {{ const batch = await codemode.batch(Array.from({{length:{calls}}}, (_,i) => () => callTool('bench::read', {{i}}))); return {{count:batch.ok.length,failed:batch.failed.length}}; }}"
    );
    let mut fresh = Vec::new();
    let mut pooled = Vec::new();
    let mut sequential = Vec::new();
    for _ in 0..samples {
        if started.elapsed() > Duration::from_mins(1) {
            return Err("benchmark exceeded 60 second total budget".into());
        }
        let host = Host::new(&runner, latency as u64, concurrency);
        fresh.push(batch(&host, &code, calls).await?);
        host.pool.shutdown().await;
    }
    let host = Host::new(&runner, latency as u64, concurrency);
    let warmup_runs = std::env::var("LABBY_CODE_MODE_POOL_SIZE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 16);
    for _ in 0..warmup_runs {
        batch(&host, &code, calls).await?;
    }
    for _ in 0..samples {
        if started.elapsed() > Duration::from_mins(1) {
            return Err("benchmark exceeded 60 second total budget".into());
        }
        pooled.push(batch(&host, &code, calls).await?);
    }
    let sequential_code = format!(
        "async () => {{ for (let i=0;i<{calls};i++) {{ await callTool('bench::read', {{i}}); }} return {{count:{calls},failed:0}}; }}"
    );
    for _ in 0..samples {
        sequential.push(batch(&host, &sequential_code, calls).await?);
    }
    host.pool.shutdown().await;
    let project = |values: &[Value]| {
        values
            .iter()
            .map(|value| value["wall_ms"].as_f64().unwrap())
            .collect::<Vec<_>>()
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"workload":{"snippets":snippets,"calls_per_execution":calls,"total_host_calls_including_warmup":(samples*3+warmup_runs)*calls,"host_admission_concurrency":concurrency,"synthetic_host_latency_ms":latency,"runner":runner,"build_profile":if cfg!(debug_assertions){"debug"}else{"release"},"pool_size_env":std::env::var("LABBY_CODE_MODE_POOL_SIZE").ok()},"cold_listing":summary(&cold),"warm_listing":summary(&warm),"fresh_pool_batch":{"timing":summary(&project(&fresh)),"runs":fresh},"warm_pool_batch":{"warmup_runs":warmup_runs,"timing":summary(&project(&pooled)),"runs":pooled},"warm_pool_sequential":{"timing":summary(&project(&sequential)),"runs":sequential},"total_wall_ms":started.elapsed().as_secs_f64()*1000.0,"note":"Real hardened QuickJS subprocess and broker; synthetic host only, no network. Cold listing samples exclude child process startup. Fresh pools include first runner startup; warm pools are primed. Small-sample p95 is descriptive."})
        )?
    );
    Ok(())
}
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| -> Box<dyn Error> { Box::new(e) })
        .and_then(|runtime| {
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_mins(1), run(&args))
                    .await
                    .unwrap_or_else(|_| Err("benchmark exceeded one-minute budget".into()))
            })
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
