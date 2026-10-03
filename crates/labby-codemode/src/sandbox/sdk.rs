//! SDK adapter for the offline disposable-workload spike.
use super::{SandboxProfile, SandboxRun, error};
use crate::error::ToolError;
use microsandbox::sandbox::{PullPolicy, SecurityProfile};
use microsandbox::{ExecEvent, LocalBackend, Sandbox};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

static ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Drop signals cancellation; the owned worker continues only to finish cleanup.
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Execute through the pinned local SDK. No SDK auto-installer is invoked.
pub(super) async fn run(profile: SandboxProfile, request: SandboxRun) -> Result<Value, ToolError> {
    request.validate(&profile)?;
    let permit = ADMISSION
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| error("resource_limit", "sandbox workload admission is full"))?;
    let cancel = CancellationToken::new();
    let guard = CancelOnDrop(cancel.clone());
    let worker = start_worker(profile, request, permit, cancel);
    let result = worker
        .await
        .map_err(|_| error("internal_error", "sandbox worker failed"))?;
    drop(guard);
    result
}

fn start_worker(
    profile: SandboxProfile,
    request: SandboxRun,
    permit: OwnedSemaphorePermit,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result<Value, ToolError>> {
    tokio::spawn(async move {
        let local = tokio::time::timeout(Duration::from_secs(5), LocalBackend::new())
            .await
            .map_err(|_| error("sandbox_unavailable", "local MSB initialization timed out"))?
            .map_err(|_| error("sandbox_unavailable", "local MSB initialization failed"))?;
        let backend: Arc<dyn microsandbox::Backend> = Arc::new(local);
        microsandbox::with_backend(backend, async move {
            let result = run_owned(profile, request, cancel).await;
            if matches!(&result, Err(ToolError::Sdk { sdk_kind, .. }) if sdk_kind == "cleanup_failed" || sdk_kind == "sandbox_start_failed") {
                // Quarantine capacity on unconfirmed cleanup; do not admit an
                // unbounded succession of potentially orphaned guests.
                permit.forget();
            } else { drop(permit); }
            result
        })
        .await
    })
}

async fn run_owned(
    profile: SandboxProfile,
    request: SandboxRun,
    cancel: CancellationToken,
) -> Result<Value, ToolError> {
    let name = format!("labby-workload-{}", ulid::Ulid::new());
    let started = std::time::Instant::now();
    let builder = Sandbox::builder(&name)
        .image(profile.image.as_str())
        .pull_policy(PullPolicy::Never)
        .cpus(profile.cpus)
        .memory(profile.memory_mib)
        .root_disk_with(|disk| disk.tmpfs().size(128_u32))
        .user("65534:65534")
        .security(SecurityProfile::Restricted)
        .disable_network()
        .max_duration(60)
        .label("owner", "labby-codemode-spike");
    // Finish or time out startup before honoring cancellation, so the worker
    // retains responsibility for the named guest even if its caller disappears.
    let created = tokio::time::timeout(Duration::from_secs(15), builder.create()).await;
    let sb = match created {
        Ok(Ok(sb)) => sb,
        _failure => {
            #[cfg(test)]
            match &_failure {
                Ok(Err(err)) => eprintln!("MSB startup diagnostic: {err}"),
                Err(err) => eprintln!("MSB startup timeout: {err}"),
                _ => {}
            }
            if let Ok(Ok(handle)) =
                tokio::time::timeout(Duration::from_secs(5), Sandbox::get(&name)).await
            {
                drop(tokio::time::timeout(Duration::from_secs(5), handle.destroy()).await);
            }
            return Err(error(
                "sandbox_start_failed",
                format!(
                    "sandbox startup failed; inspect owned guest {name} if cleanup is unconfirmed"
                ),
            ));
        }
    };
    let work = execute(&sb, &request);
    let budget = Duration::from_millis(request.timeout_ms.unwrap_or(profile.timeout_ms));
    let output = tokio::select! {
        biased;
        ()=cancel.cancelled()=>Err(error("cancelled","sandbox invocation cancelled")),
        result=tokio::time::timeout(budget,work)=>result.unwrap_or_else(|_|Err(error("timeout","sandbox workload deadline expired"))),
    };
    // Destroy kills/stops and removes only this SDK handle's stable identity.
    let cleanup = tokio::time::timeout(Duration::from_secs(5), sb.destroy()).await;
    if !matches!(cleanup, Ok(Ok(()))) {
        tracing::warn!(sandbox=%name,"Code Mode workload cleanup unconfirmed");
        return Err(error(
            "cleanup_failed",
            format!("cleanup unconfirmed for owned guest {name}"),
        ));
    }
    let mut result = output?;
    result["sandbox"] = json!(name);
    result["image"] = json!(profile.image);
    result["elapsed_ms"] = json!(started.elapsed().as_millis());
    result["cleanup_confirmed"] = json!(true);
    result["network"] = json!("disabled");
    result["projected_files"] = json!(request.files.keys().collect::<Vec<_>>());
    Ok(result)
}

async fn execute(sb: &Sandbox, request: &SandboxRun) -> Result<Value, ToolError> {
    sb.fs()
        .mkdir("/work")
        .await
        .map_err(|_| error("projection_failed", "guest work directory creation failed"))?;
    for (path, content) in &request.files {
        sb.fs()
            .write(&format!("/work/{path}"), content)
            .await
            .map_err(|_| error("projection_failed", "guest file projection failed"))?;
    }
    let mut exec = sb
        .exec_stream_with(&request.command[0], |options| {
            options
                .args(&request.command[1..])
                .user("65534:65534")
                .stdin_null()
        })
        .await
        .map_err(|_| error("execution_failed", "guest command could not start"))?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Some(event) = exec.recv().await {
        match event {
            ExecEvent::Stdout(bytes) => append_output(&mut stdout, stderr.len(), &bytes)?,
            ExecEvent::Stderr(bytes) => append_output(&mut stderr, stdout.len(), &bytes)?,
            ExecEvent::Exited { code } => {
                return Ok(
                    json!({"ok":code==0,"exit_code":code,"stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr)}),
                );
            }
            ExecEvent::Failed(_) => {
                return Err(error("execution_failed", "guest process spawn failed"));
            }
            _ => {}
        }
    }
    Err(error(
        "execution_failed",
        "guest stream ended without exit status",
    ))
}

fn append_output(target: &mut Vec<u8>, other_len: usize, bytes: &[u8]) -> Result<(), ToolError> {
    if bytes.len() > (64 * 1024_usize).saturating_sub(target.len() + other_len) {
        return Err(error(
            "output_limit",
            "guest output exceeded 64 KiB; invocation stopped",
        ));
    }
    target.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodeModeBroker, CodeModeCaller, CodeModeConfig, CodeModeSurface, ToolScope};

    #[test]
    fn combined_output_limit_is_checked_before_allocation() {
        let mut stdout = vec![0; 64 * 1024 - 2];
        assert!(append_output(&mut stdout, 1, &[1]).is_ok());
        let before = stdout.len();
        assert!(append_output(&mut stdout, 1, &[2]).is_err());
        assert_eq!(stdout.len(), before);
    }

    /// Explicit opt-in only: creates one disposable guest using a cached image.
    #[tokio::test]
    #[ignore = "requires MSB runtime, cached Node image and operator profile"]
    async fn sandbox_live_codemode_projection() {
        let profile: SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("operator profile"),
        )
        .expect("profile JSON");
        let request = json!({"image":profile.image,"command":["node","/work/analyze.js"],"timeout_ms":1000,"files":{"analyze.js":"console.log(JSON.stringify({answer:42,uid:process.getuid(),interfaces:Object.keys(require('os').networkInterfaces()),hostTokenPresent:Boolean(process.env.GH_TOKEN || process.env.GITHUB_TOKEN)}))"}});
        let code = format!("async () => {{ return await codemode.sandbox.run({request}); }}");
        let broker: CodeModeBroker<'_, crate::host::NoopHost> = CodeModeBroker::new(None);
        let response = broker
            .execute(
                &code,
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                CodeModeConfig::default(),
                ToolScope::default(),
                None,
            )
            .await
            .expect("Code Mode execution");
        let serialized = serde_json::to_value(response).expect("response");
        println!("LIVE_RECEIPT={serialized}");
        assert!(!serialized["calls"].as_array().expect("calls").is_empty());
        let result = &serialized["result"];
        assert_eq!(result["cleanup_confirmed"], true);
        assert_eq!(result["exit_code"], 0);
        let guest: Value =
            serde_json::from_str(result["stdout"].as_str().expect("stdout")).expect("guest JSON");
        assert_eq!(guest["answer"], 42);
        assert_eq!(guest["uid"], 65534);
        assert_eq!(guest["hostTokenPresent"], false);
        assert!(
            guest["interfaces"]
                .as_array()
                .expect("interfaces")
                .iter()
                .all(|name| name == "lo")
        );
    }

    #[test]
    fn dropping_invocation_signals_owned_worker() {
        let token = CancellationToken::new();
        let guard = CancelOnDrop(token.clone());
        drop(guard);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_cancellation_cleans_guest() {
        let profile: SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("operator profile"),
        )
        .expect("profile");
        let request: SandboxRun = serde_json::from_value(json!({"image":profile.image,"command":["node","-e","setTimeout(()=>{},10000)"],"files":{}})).expect("request");
        let cancel = CancellationToken::new();
        let guard = CancelOnDrop(cancel.clone());
        let permit = Arc::new(Semaphore::new(1))
            .acquire_owned()
            .await
            .expect("permit");
        let worker = start_worker(profile, request, permit, cancel);
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(guard);
        let result = tokio::time::timeout(Duration::from_secs(26), worker)
            .await
            .expect("owned cleanup deadline")
            .expect("worker");
        assert!(matches!(result, Err(ToolError::Sdk { sdk_kind, .. }) if sdk_kind == "cancelled"));
    }

    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_output_overflow_cleans_guest() {
        let profile: SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("operator profile"),
        )
        .expect("profile");
        let request: SandboxRun = serde_json::from_value(json!({"image":profile.image,"command":["node","-e","process.stdout.write('x'.repeat(70000));setTimeout(()=>{},10000)"],"timeout_ms":2000,"files":{}})).expect("request");
        let result = run(profile, request).await;
        assert!(
            matches!(result, Err(ToolError::Sdk { sdk_kind, .. }) if sdk_kind == "output_limit")
        );
    }

    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_timeout_cleans_guest() {
        let profile: SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("operator profile"),
        )
        .expect("profile");
        let request:SandboxRun=serde_json::from_value(json!({"image":profile.image,"command":["node","-e","setTimeout(()=>{},10000)"],"timeout_ms":100,"files":{}})).expect("request");
        let result = run(profile, request).await;
        assert!(matches!(result,Err(ToolError::Sdk{sdk_kind,..}) if sdk_kind=="timeout"));
    }
}
