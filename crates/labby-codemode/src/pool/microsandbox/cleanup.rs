use super::*;

pub(super) fn failed_cleanups() -> &'static Mutex<HashMap<CleanupIdentity, bool>> {
    FAILED_CLEANUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn cleanup_failed(identity: &CleanupIdentity, counted: bool) {
    failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(identity.clone())
        .and_modify(|was_counted| *was_counted |= counted)
        .or_insert(counted);
}

fn decrement_active_sandbox() {
    let _ = ACTIVE_SANDBOXES.try_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |active| Some(active.saturating_sub(1)),
    );
}

pub(super) fn take_failed_cleanup_counted(identity: &CleanupIdentity) -> bool {
    failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(identity)
        .unwrap_or(false)
}

pub(super) fn resolve_failed_cleanup(identity: &CleanupIdentity) {
    if take_failed_cleanup_counted(identity) {
        decrement_active_sandbox();
    }
}

pub(super) fn complete_cleanup(
    identity: &CleanupIdentity,
    counted: bool,
    failed_cleanup_recorded: bool,
) {
    if failed_cleanup_recorded {
        resolve_failed_cleanup(identity);
    } else if counted {
        decrement_active_sandbox();
    }
}

pub(super) fn cleanup_executor() -> &'static mpsc::SyncSender<CleanupJob> {
    CLEANUP_EXECUTOR.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(CLEANUP_QUEUE_CAPACITY);
        let receiver = std::sync::Arc::new(Mutex::new(receiver));
        for index in 0..CLEANUP_WORKERS {
            let receiver = std::sync::Arc::clone(&receiver);
            if let Err(error) = std::thread::Builder::new()
                .name(format!("labby-msb-cleanup-{index}"))
                .spawn(move || {
                    loop {
                        let job = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        let Ok(job) = job else { break };
                        run_fallback_cleanup(job);
                    }
                })
            {
                tracing::error!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "microsandbox.cleanup_executor",
                    kind = "cleanup_spawn_failed",
                    worker = index,
                    %error,
                    "failed to start bounded Microsandbox cleanup worker"
                );
            }
        }
        sender
    })
}

fn run_fallback_cleanup(job: CleanupJob) {
    let started = Instant::now();
    let mut child = std::process::Command::new(&job.identity.executable)
        .args(["remove", "--quiet", "--force", &job.identity.name])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let success = match child.as_mut() {
        Ok(child) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Ok(None) => {
                        drop(child.kill());
                        drop(child.wait());
                        break false;
                    }
                    Err(_) => break false,
                }
            }
        }
        Err(_) => false,
    };
    if success {
        complete_cleanup(&job.identity, job.counted, job.failed_cleanup_recorded);
        log_lifecycle(
            "microsandbox.remove_fallback",
            &job.identity.name,
            started,
            "success",
            None,
        );
    } else {
        cleanup_failed(&job.identity, job.counted);
        log_lifecycle(
            "microsandbox.remove_fallback",
            &job.identity.name,
            started,
            "failed",
            Some("cleanup_failed"),
        );
    }
}
