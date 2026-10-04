//! Persisted guest ownership and conservative recovery before admission.
use super::error;
use crate::error::ToolError;
use microsandbox::sandbox::SandboxId;
use microsandbox::{MicrosandboxError, Sandbox};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit};

pub(super) const OWNER: &str = "labby-codemode-workload-v1";
static QUARANTINE: OnceLock<Mutex<HashMap<String, Quarantined>>> = OnceLock::new();
static RECOVERY: Mutex<()> = Mutex::const_new(());

struct Quarantined {
    _permit: OwnedSemaphorePermit,
    identity: Option<SandboxId>,
}

fn ledger() -> &'static Mutex<HashMap<String, Quarantined>> {
    QUARANTINE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn name() -> String {
    format!(
        "labby-workload-{}-{}",
        std::process::id(),
        ulid::Ulid::new()
    )
}

fn owner_pid(name: &str) -> Option<u32> {
    let (pid, nonce) = name.strip_prefix("labby-workload-")?.split_once('-')?;
    nonce.parse::<ulid::Ulid>().ok()?;
    pid.parse::<u32>().ok().filter(|pid| *pid > 0)
}

#[cfg(unix)]
fn is_dead(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        // An unrepresentable owner cannot be probed: preserve its guest.
        return false;
    };
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None),
        Err(nix::errno::Errno::ESRCH)
    )
}
#[cfg(not(unix))]
fn is_dead(_pid: u32) -> bool {
    false
}

pub(super) async fn quarantine(
    name: String,
    permit: OwnedSemaphorePermit,
    identity: Option<SandboxId>,
) {
    ledger().lock().await.insert(
        name,
        Quarantined {
            _permit: permit,
            identity,
        },
    );
}

/// List only our versioned owner label. Missing/old ownership is never inferred.
/// The SDK handle fences destruction against replacement by stable identity.
pub(super) async fn reconcile() -> Result<(), ToolError> {
    tokio::time::timeout(Duration::from_secs(12), reconcile_inner())
        .await
        .map_err(|_| error("cleanup_failed", "workload recovery deadline expired"))?
}

async fn reconcile_inner() -> Result<(), ToolError> {
    let _serial = RECOVERY.lock().await;
    let known = ledger()
        .lock()
        .await
        .iter()
        .filter_map(|(name, record)| {
            record
                .identity
                .clone()
                .map(|identity| (name.clone(), identity))
        })
        .collect::<Vec<_>>();
    for (name, identity) in known {
        if matches!(
            Sandbox::get(&name).await,
            Err(MicrosandboxError::SandboxNotFound(_))
        ) {
            let mut records = ledger().lock().await;
            if records
                .get(&name)
                .is_some_and(|record| record.identity.as_ref() == Some(&identity))
            {
                // This identity was observed after creation: absence proves
                // terminal removal, unlike an uncertain startup record.
                drop(records.remove(&name));
            }
        }
    }
    let mut cursor = None;
    for _ in 0..16 {
        let page = Sandbox::list_with(|list| {
            let list = list.label("owner", OWNER).limit(32);
            match cursor.take() {
                Some(cursor) => list.cursor(cursor),
                None => list,
            }
        })
        .await
        .map_err(|_| error("cleanup_failed", "owned workload inventory unavailable"))?;
        for handle in page.sandboxes {
            let Some(pid) = owner_pid(handle.name()) else {
                continue;
            };
            let records = ledger().lock().await;
            let record = records.get(handle.name());
            if record
                .and_then(|record| record.identity.as_ref())
                .is_some_and(|identity| *identity != handle.id())
            {
                return Err(error(
                    "cleanup_failed",
                    "quarantined workload identity changed; preserving replacement",
                ));
            }
            let quarantined = record.is_some();
            drop(records);
            if !is_dead(pid) && !(pid == std::process::id() && quarantined) {
                continue;
            }
            let removed = tokio::time::timeout(Duration::from_secs(5), handle.destroy()).await;
            if !matches!(removed, Ok(Ok(()))) {
                return Err(error(
                    "cleanup_failed",
                    "owned workload recovery removal unconfirmed",
                ));
            }
            if !matches!(
                Sandbox::get(handle.name()).await,
                Err(MicrosandboxError::SandboxNotFound(_))
            ) {
                return Err(error(
                    "cleanup_failed",
                    "owned workload absence unconfirmed",
                ));
            }
            // Dropping this counted permit is the only capacity recovery path.
            // Unknown startup records absent from the inventory remain quarantined:
            // absence alone does not prove a delayed create cannot still publish.
            drop(ledger().lock().await.remove(handle.name()));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(());
        }
    }
    Err(error(
        "cleanup_failed",
        "owned workload recovery inventory exceeds limit",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_recovery_removes_dead_owner_preserves_live_owner() {
        let profile: crate::sandbox::SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("operator profile"),
        )
        .expect("profile");
        let backend: std::sync::Arc<dyn microsandbox::Backend> = std::sync::Arc::new(
            microsandbox::LocalBackend::new()
                .await
                .expect("local backend"),
        );
        microsandbox::with_backend(backend, async move {
            let mut owner = tokio::process::Command::new("/usr/bin/true")
                .spawn()
                .expect("disposable owner");
            let dead_pid = owner.id().expect("owner pid");
            owner.wait().await.expect("owner exited");
            assert!(is_dead(dead_pid));
            let dead_name = format!("labby-workload-{dead_pid}-{}", ulid::Ulid::new());
            let live_name = name();
            let create = |name: &str| {
                Sandbox::builder(name)
                    .image(profile.image.as_str())
                    .pull_policy(microsandbox::sandbox::PullPolicy::Never)
                    .cpus(1_u8)
                    .memory(256_u32)
                    .root_disk_with(|disk| disk.tmpfs().size(128_u32))
                    .user("65534:65534")
                    .disable_network()
                    .max_duration(60)
                    .label("owner", OWNER)
            };
            let dead = create(&dead_name).create().await.expect("dead-owner guest");
            let live = create(&live_name).create().await.expect("live-owner guest");
            let result = reconcile().await;
            let dead_absent = matches!(
                Sandbox::get(&dead_name).await,
                Err(MicrosandboxError::SandboxNotFound(_))
            );
            let live_present = Sandbox::get(&live_name).await.is_ok();
            let quarantine_name = name();
            let quarantined = create(&quarantine_name)
                .create()
                .await
                .expect("quarantine guest");
            let pool = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            quarantine(
                quarantine_name.clone(),
                pool.clone().acquire_owned().await.expect("permit"),
                Some(dead.id()),
            )
            .await;
            assert_eq!(pool.available_permits(), 0);
            let replacement_rejected = reconcile().await.is_err();
            let replacement_preserved = Sandbox::get(&quarantine_name).await.is_ok();
            ledger()
                .lock()
                .await
                .get_mut(&quarantine_name)
                .expect("quarantine record")
                .identity = Some(quarantined.id());
            let reclaimed = reconcile().await;
            let quarantine_absent = matches!(
                Sandbox::get(&quarantine_name).await,
                Err(MicrosandboxError::SandboxNotFound(_))
            );
            drop(quarantined.destroy().await);
            // Always clean task-owned guests before assertions.
            drop(dead.destroy().await);
            live.destroy().await.expect("live test guest cleanup");
            result.expect("recovery");
            assert!(dead_absent);
            assert!(live_present);
            let uncertain_name = name();
            quarantine(
                uncertain_name.clone(),
                pool.clone()
                    .acquire_owned()
                    .await
                    .expect("uncertain permit"),
                None,
            )
            .await;
            reconcile().await.expect("absent startup inventory");
            let uncertain_retained = pool.available_permits() == 0;
            drop(ledger().lock().await.remove(&uncertain_name));
            quarantine(
                quarantine_name.clone(),
                pool.clone()
                    .acquire_owned()
                    .await
                    .expect("late cleanup permit"),
                Some(quarantined.id()),
            )
            .await;
            reconcile().await.expect("known absent cleanup recovery");
            let late_cleanup_recovered = pool.available_permits() == 1;
            let startup_name = name();
            let original = create(&startup_name).create().await.expect("startup guest");
            let original_handle = Sandbox::get(&startup_name)
                .await
                .expect("observed startup identity");
            let original_id = original_handle.id();
            original.destroy().await.expect("remove original");
            drop(original);
            let replacement = create(&startup_name)
                .create()
                .await
                .expect("replacement guest");
            let (cleaned, startup_identity) =
                crate::sandbox::sdk::cleanup_startup(original_handle).await;
            quarantine(
                startup_name.clone(),
                pool.clone().acquire_owned().await.expect("startup permit"),
                Some(startup_identity.clone()),
            )
            .await;
            let startup_replacement_rejected = reconcile().await.is_err();
            let startup_replacement_preserved = Sandbox::get(&startup_name).await.is_ok();
            replacement
                .destroy()
                .await
                .expect("replacement test cleanup");
            reconcile()
                .await
                .expect("removed startup identity capacity recovery");
            assert!(!cleaned);
            assert_eq!(startup_identity, original_id);
            assert!(startup_replacement_rejected);
            assert!(startup_replacement_preserved);
            assert!(uncertain_retained);
            assert!(late_cleanup_recovered);
            assert!(replacement_rejected);
            assert!(replacement_preserved);
            reclaimed.expect("quarantine recovery");
            assert!(quarantine_absent);
            assert_eq!(pool.available_permits(), 1);
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "subprocess fixture; invoked only by crash recovery test"]
    async fn crash_fixture_child() {
        if std::env::var("LABBY_MSB_CRASH_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        let profile: crate::sandbox::SandboxProfile = serde_json::from_str(
            &std::env::var("LABBY_CODE_MODE_SANDBOX_PROFILE_JSON").expect("profile"),
        )
        .expect("profile JSON");
        let backend: std::sync::Arc<dyn microsandbox::Backend> = std::sync::Arc::new(
            microsandbox::LocalBackend::new()
                .await
                .expect("local backend"),
        );
        microsandbox::with_backend(backend, async move {
            let name = name();
            let starting = std::env::var("LABBY_MSB_CRASH_STARTING").as_deref() == Ok("1");
            if starting {
                println!("GUEST_READY={name}");
                std::io::Write::flush(&mut std::io::stdout()).expect("startup signal");
            }
            let _guest = Sandbox::builder(&name)
                .image(profile.image.as_str())
                .pull_policy(microsandbox::sandbox::PullPolicy::Never)
                .cpus(1_u8)
                .memory(256_u32)
                .root_disk_with(|disk| disk.tmpfs().size(128_u32))
                .disable_network()
                .user("65534:65534")
                .max_duration(60)
                .label("owner", OWNER)
                .create()
                .await
                .expect("crash fixture guest");
            if !starting {
                println!("GUEST_READY={name}");
                std::io::Write::flush(&mut std::io::stdout()).expect("ready signal");
            }
            std::future::pending::<()>().await;
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_process_crash_recovery() {
        crash_recovery(false).await;
    }

    #[tokio::test]
    #[ignore = "requires MSB runtime and cached Node image"]
    async fn sandbox_live_interrupted_startup_recovery() {
        crash_recovery(true).await;
    }

    async fn crash_recovery(starting: bool) {
        use tokio::io::AsyncBufReadExt;
        let backend: std::sync::Arc<dyn microsandbox::Backend> = std::sync::Arc::new(
            microsandbox::LocalBackend::new()
                .await
                .expect("local backend"),
        );
        let mut child =
            tokio::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "sandbox::recovery::tests::crash_fixture_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("LABBY_MSB_CRASH_FIXTURE", "1")
                .env("LABBY_MSB_CRASH_STARTING", if starting { "1" } else { "0" })
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("fixture child");
        let pid = child.id().expect("child pid");
        let mut lines = tokio::io::BufReader::new(child.stdout.take().expect("stdout")).lines();
        let ready = tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(line) = lines.next_line().await.expect("ready output") {
                if let Some(name) = line.split("GUEST_READY=").nth(1) {
                    return Some(name.trim().to_string());
                }
            }
            None
        })
        .await;
        let observed_startup = if starting {
            let name = ready.as_ref().ok().and_then(|name| name.as_ref());
            microsandbox::with_backend(backend.clone(), async {
                let Some(name) = name else {
                    return false;
                };
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if let Ok(handle) = Sandbox::get(name).await {
                            return matches!(
                                handle.status_snapshot(),
                                microsandbox::sandbox::SandboxStatus::Created
                                    | microsandbox::sandbox::SandboxStatus::Starting
                            );
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .unwrap_or(false)
            })
            .await
        } else {
            true
        };
        child.kill().await.expect("kill fixture owner");
        child.wait().await.expect("reap fixture owner");
        assert!(is_dead(pid));
        let ready_name = ready.ok().flatten();
        microsandbox::with_backend(backend, async move {
            let result = reconcile().await;
            let name = ready_name.expect("fixture readiness deadline after recovery");
            let absent = matches!(
                Sandbox::get(&name).await,
                Err(MicrosandboxError::SandboxNotFound(_))
            );
            if let Ok(handle) = Sandbox::get(&name).await {
                drop(handle.destroy().await);
            }
            result.expect("crash recovery");
            assert!(absent);
            assert!(
                observed_startup,
                "owner must be killed while SDK startup is in progress"
            );
        })
        .await;
    }

    #[test]
    fn ownership_requires_valid_pid_and_nonce() {
        assert_eq!(owner_pid(&name()), Some(std::process::id()));
        for bad in [
            "labby-workload-old",
            "labby-workload-0-01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "labby-workload-123-not-a-nonce",
            "other-123-01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ] {
            assert_eq!(owner_pid(bad), None);
        }
        assert!(!is_dead(std::process::id()));
    }
    #[tokio::test]
    async fn quarantine_retains_capacity_until_confirmed_removal() {
        let pool = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let name = name();
        quarantine(
            name.clone(),
            pool.clone().acquire_owned().await.expect("permit"),
            None,
        )
        .await;
        assert_eq!(pool.available_permits(), 0);
        drop(ledger().lock().await.remove(&name));
        assert_eq!(pool.available_permits(), 1);
    }

    #[test]
    fn unrepresentable_owner_pids_are_not_proven_dead() {
        for pid in [i32::MAX as u32 + 1, u32::MAX] {
            let name = format!("labby-workload-{pid}-01ARZ3NDEKTSV4RRFFQ69G5FAV");
            let owner = owner_pid(&name).expect("syntactically valid owner");
            assert!(!is_dead(owner), "unprovable ownership must preserve {name}");
        }
        assert!(!is_dead(std::process::id()));
    }
}
