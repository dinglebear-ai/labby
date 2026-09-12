//! Host executable updates and opt-in macOS scheduling.

use std::{fs, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const LABEL: &str = "net.labby.auto-update";
const ASSET: &str = "lab-aarch64-apple-darwin.tar.gz";
const REPO: &str = "dinglebear-ai/labby";
const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install.sh");

#[derive(Deserialize)]
struct Asset {
    name: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

fn version(text: &str) -> Result<[u64; 3]> {
    let text = text.trim().strip_prefix("labby ").unwrap_or(text.trim());
    let text = text.strip_prefix('v').unwrap_or(text);
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        bail!("Expected a stable Labby version, got {text:?}");
    }
    Ok([parts[0].parse()?, parts[1].parse()?, parts[2].parse()?])
}

fn select_release(releases: &[Release], current: [u64; 3]) -> Option<&str> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter(|r| {
            [ASSET.to_string(), format!("{ASSET}.sha256")]
                .iter()
                .all(|name| r.assets.iter().any(|a| &a.name == name))
        })
        .filter_map(|r| {
            version(&r.tag_name)
                .ok()
                .filter(|v| *v > current)
                .map(|v| (v, r.tag_name.as_str()))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, tag)| tag)
}

pub(crate) fn require_macos() -> Result<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        bail!("Automatic updates currently support macOS Apple Silicon only");
    }
    Ok(())
}

fn installer_command(script: &Path, tag: &str, directory: &Path) -> Command {
    let mut command = Command::new("sh");
    command.arg(script);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LABBY_INSTALL_") {
            command.env_remove(key);
        }
    }
    command
        .env("LABBY_INSTALL_VERSION", tag)
        .env("LABBY_INSTALL_DIR", directory)
        .env("LABBY_INSTALL_REPO", REPO)
        .env("LABBY_ALLOW_SOURCE_FALLBACK", "0");
    command
}

// Keep exclusion ownership until cancellation has killed and reaped the
// installer. A detached blocking task can outlive the service's runtime.
struct InstallerProcess {
    #[cfg(unix)]
    child: nix::unistd::Pid,
    #[cfg(not(unix))]
    child: std::process::Child,
    _lock: fs::File,
    finished: bool,
}

impl InstallerProcess {
    fn try_wait(&mut self) -> Result<Option<bool>> {
        #[cfg(unix)]
        {
            use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
            Ok(match waitpid(self.child, Some(WaitPidFlag::WNOHANG))? {
                WaitStatus::Exited(_, status) => Some(status == 0),
                WaitStatus::Signaled(..) => Some(false),
                _ => None,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(self.child.try_wait()?.map(|status| status.success()))
        }
    }
}

impl Drop for InstallerProcess {
    fn drop(&mut self) {
        if !self.finished {
            #[cfg(unix)]
            {
                let _ = nix::sys::signal::killpg(self.child, nix::sys::signal::Signal::SIGKILL);
                while nix::sys::wait::waitpid(self.child, None) == Err(nix::errno::Errno::EINTR) {}
            }
            #[cfg(not(unix))]
            {
                drop(self.child.kill());
                drop(self.child.wait());
            }
        }
    }
}

#[cfg(unix)]
fn spawn_installer(
    command: &mut Command,
    stderr: &fs::File,
    lock: &fs::File,
) -> Result<nix::unistd::Pid> {
    use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawnp};
    use std::ffi::{CString, OsString};
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};

    let mut environment: std::collections::BTreeMap<OsString, OsString> =
        std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            environment.insert(key.to_owned(), value.to_owned());
        } else {
            environment.remove(key);
        }
    }
    let environment = environment
        .into_iter()
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            CString::new(key.as_bytes())
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let args = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| CString::new(arg.as_bytes()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let null = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let mut actions = PosixSpawnFileActions::init()?;
    actions.add_dup2(null.as_raw_fd(), 0)?;
    actions.add_dup2(null.as_raw_fd(), 1)?;
    actions.add_dup2(stderr.as_raw_fd(), 2)?;
    // Only the installer inherits this descriptor. Unlike clearing CLOEXEC in
    // the parent, child-local dup2 cannot leak the lock to unrelated spawns.
    // Descendants retain exclusion even if the service is killed or crashes.
    // macOS leaves CLOEXEC set when dup2's source and target are identical.
    let lock_copy = lock.try_clone()?;
    let lock_fd = if lock.as_raw_fd() == 9 {
        lock_copy.as_raw_fd()
    } else {
        lock.as_raw_fd()
    };
    actions.add_dup2(lock_fd, 9)?;
    let mut attributes = PosixSpawnAttr::init()?;
    attributes.set_pgroup(nix::unistd::Pid::from_raw(0))?;
    attributes.set_flags(PosixSpawnFlags::POSIX_SPAWN_SETPGROUP)?;
    Ok(posix_spawnp(
        &args[0],
        &actions,
        &attributes,
        &args,
        &environment,
    )?)
}

#[cfg(not(unix))]
fn spawn_installer(
    command: &mut Command,
    stderr: &fs::File,
    _lock: &fs::File,
) -> Result<std::process::Child> {
    Ok(command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr.try_clone()?)
        .spawn()?)
}

async fn install_release(script: &Path, tag: &str, directory: &Path, lock: fs::File) -> Result<()> {
    use std::io::{Read, Seek};
    let mut stderr = tempfile::tempfile()?;
    let mut command = installer_command(script, tag, directory);
    let mut process = InstallerProcess {
        child: spawn_installer(&mut command, &stderr, &lock)?,
        _lock: lock,
        finished: false,
    };
    let status = tokio::time::timeout(Duration::from_mins(15), async {
        loop {
            if let Some(status) = process.try_wait()? {
                return Ok::<bool, anyhow::Error>(status);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("Verified release installation exceeded its 15-minute deadline")??;
    process.finished = true;
    if !status {
        stderr.rewind()?;
        let mut message = Vec::new();
        stderr.take(64 * 1024).read_to_end(&mut message)?;
        bail!(
            "Verified release install failed: {}",
            String::from_utf8_lossy(&message)
        );
    }
    Ok(())
}

fn acquire_update_lock(directory: &Path) -> Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(directory.join(".labby-auto-update.lock"))?;
    lock.try_lock()
        .context("Another automatic update is running")?;
    Ok(lock)
}

fn install_directory(binary: &Path) -> Result<&Path> {
    // The verified installer publishes a fixed `labby` basename. A renamed
    // executable would otherwise report success while leaving itself stale.
    if binary.file_name() != Some(std::ffi::OsStr::new("labby")) {
        bail!(
            "Automatic updates require an executable named labby; reinstall at the standard name before enabling updates"
        );
    }
    binary.parent().context("Binary has no parent directory")
}

/// Check published releases and atomically install a newer verified host binary.
pub(crate) async fn automatic(binary: &Path, dry_run: bool) -> Result<Value> {
    require_macos()?;
    let directory = install_directory(binary)?;
    let lock = acquire_update_lock(directory)?;
    let output = Command::new(binary).arg("--version").output()?;
    if !output.status.success() {
        bail!("Cannot read installed Labby version");
    }
    let current = version(std::str::from_utf8(&output.stdout)?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("labby-auto-update")
        .build()?;
    let mut response = client
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=100"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > 4 * 1024 * 1024 {
            bail!("Release catalog exceeds 4 MiB");
        }
        body.extend_from_slice(&chunk);
    }
    let releases: Vec<Release> = serde_json::from_slice(&body)?;
    let Some(tag) = select_release(&releases, current) else {
        return Ok(json!({"installed": false, "reason": "No newer stable Labby binary release"}));
    };
    if !dry_run {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("install.sh");
        fs::write(&script, INSTALL_SCRIPT)?;
        install_release(&script, tag, directory, lock).await?;
    }
    Ok(json!({"installed": !dry_run, "version": tag, "binary": binary, "dry_run": dry_run}))
}

/// Wait until the server has installed an update and needs a supervised restart.
#[cfg(unix)]
pub(crate) async fn server_update_loop() {
    let binary = match std::env::current_exe() {
        Ok(binary) => binary,
        Err(error) => {
            tracing::error!(%error, "cannot resolve server executable for automatic updates");
            std::future::pending::<()>().await;
            return;
        }
    };
    wait_for_installed_update(
        || automatic(&binary, false),
        Duration::from_mins(1),
        Duration::from_hours(24),
    )
    .await;
}

#[cfg(unix)]
async fn wait_for_installed_update<F, Fut>(mut check: F, initial: Duration, interval: Duration)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    tokio::time::sleep(initial).await;
    loop {
        match check().await {
            Ok(outcome) if outcome.get("installed").and_then(Value::as_bool) == Some(true) => {
                tracing::info!(version = ?outcome.get("version"), "verified update installed; restarting Labby server");
                return;
            }
            Ok(_) => tracing::info!("automatic update check: no newer stable release"),
            Err(error) => {
                tracing::warn!(%error, "automatic update failed; server stays running; retry in 24 hours")
            }
        }
        tokio::time::sleep(interval).await;
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn launch_agent(binary: &Path, log: &Path, path: &str) -> Result<String> {
    let binary = xml(binary.to_str().context("Binary path must be UTF-8")?);
    let log = xml(log.to_str().context("Log path must be UTF-8")?);
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>{binary}</string><string>update</string><string>--automatic</string></array>
<key>StartInterval</key><integer>86400</integer><key>RunAtLoad</key><true/>
<key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string>
<key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict>
</dict></plist>
"#,
        xml(path)
    ))
}

fn acquire_schedule_lock(plist: &Path) -> Result<fs::File> {
    fs::create_dir_all(plist.parent().context("Missing LaunchAgents directory")?)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(plist.with_extension("lock"))?;
    lock.try_lock()
        .context("Another updater schedule operation is running")?;
    Ok(lock)
}

/// Configure or inspect the per-user launchd job, which invokes this executable.
pub(crate) fn schedule(action: &str, dry_run: bool) -> Result<Value> {
    require_macos()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let home = Path::new(&home);
    let plist = home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"));
    let binary = std::env::current_exe()?;
    if dry_run {
        return Ok(json!({"action": action, "binary": binary, "plist": plist, "dry_run": true}));
    }
    let _schedule_lock = acquire_schedule_lock(&plist)?;
    let uid = Command::new("id").arg("-u").output()?;
    if !uid.status.success() {
        bail!("Cannot determine launchd user ID");
    }
    let domain = format!("gui/{}", std::str::from_utf8(&uid.stdout)?.trim());
    let loaded = Command::new("launchctl")
        .args(["print", &format!("{domain}/{LABEL}")])
        .output()?
        .status
        .success();
    if action == "status" {
        return Ok(json!({"enabled": loaded, "plist": plist}));
    }
    if !matches!(action, "enable" | "disable") {
        bail!("Unknown schedule action {action}");
    }
    let control = |operation: &str| -> Result<()> {
        let mut command = Command::new("launchctl");
        command.arg(operation);
        if operation == "bootout" {
            command.arg(format!("{domain}/{LABEL}"));
        } else {
            command.arg(&domain).arg(&plist);
        }
        if !command.status()?.success() {
            bail!("launchctl {operation} failed for {}", plist.display());
        }
        Ok(())
    };
    if action == "disable" {
        if loaded {
            control("bootout")?;
        }
        match fs::remove_file(&plist) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(json!({"enabled": false}));
    }
    // Prepare the replacement before changing the working schedule.
    let log_dir = home.join("Library/Logs/Labby");
    let content = launch_agent(
        &binary,
        &log_dir.join("auto-update.log"),
        &std::env::var("PATH")?,
    )?;
    if !Command::new("gh")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        bail!("GitHub CLI (gh) is required to verify release attestations");
    }
    fs::create_dir_all(plist.parent().context("Missing LaunchAgents directory")?)?;
    fs::create_dir_all(log_dir)?;
    replace_schedule(&plist, content.as_bytes(), loaded, control)?;
    Ok(json!({"enabled": true, "binary": binary, "interval_seconds": 86400, "plist": plist}))
}

/// Restore the previous on-disk and loaded schedule if activation fails.
fn replace_schedule(
    plist: &Path,
    content: &[u8],
    loaded: bool,
    mut control: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    use std::io::Write;

    let previous = match fs::read(plist) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !loaded => None,
        Err(error) => return Err(error).context("Cannot preserve existing update schedule"),
    };
    let stage = |content: &[u8]| -> Result<tempfile::NamedTempFile> {
        let mut file = tempfile::NamedTempFile::new_in(
            plist.parent().context("Missing LaunchAgents directory")?,
        )?;
        file.write_all(content)?;
        file.as_file().sync_all()?;
        Ok(file)
    };
    let replacement = stage(content)?;
    // Stage rollback bytes too, so an unwritable directory cannot stop the old
    // job before either complete file is ready for atomic publication.
    let backup = previous.as_deref().map(stage).transpose()?;
    if loaded {
        control("bootout")?;
    }
    let activation = (|| -> Result<()> {
        replacement.persist(plist)?;
        control("bootstrap")
    })();
    if let Err(error) = activation {
        let rollback = (|| -> Result<()> {
            if let Some(backup) = backup {
                backup.persist(plist)?;
            } else {
                match fs::remove_file(plist) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if loaded {
                control("bootstrap")?;
            }
            Ok(())
        })();
        if let Err(rollback_error) = rollback {
            bail!(
                "Update schedule activation failed: {error:#}; restoration also failed: {rollback_error:#}"
            );
        }
        return Err(error).context("Update schedule activation failed; previous schedule restored");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_lock_child() {
        let Some(path) = std::env::var_os("LABBY_TEST_SCHEDULE_LOCK") else {
            return;
        };
        assert!(acquire_schedule_lock(Path::new(&path)).is_err());
    }

    #[test]
    fn schedule_lock_excludes_other_processes_and_survives_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("updater.plist");
        let lock = acquire_schedule_lock(&plist).unwrap();
        fs::write(&plist, "first").unwrap();
        fs::remove_file(&plist).unwrap();
        fs::write(&plist, "replacement").unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "self_update::tests::schedule_lock_child"])
            .env("LABBY_TEST_SCHEDULE_LOCK", &plist)
            .status()
            .unwrap();
        assert!(child.success());
        drop(lock);
        assert!(acquire_schedule_lock(&plist).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_installer_is_reaped_before_unlocking_and_cannot_write_later() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("install.sh");
        fs::write(
            &script,
            r#"
mkdir -p "$LABBY_INSTALL_DIR/.labby-install/activation-journal"
printf 'staged' > "$LABBY_INSTALL_DIR/.labby-install/activation-journal/state"
(sleep 1; touch "$LABBY_INSTALL_DIR/late-write") &
echo $$ > "$LABBY_INSTALL_DIR/installer.pid"
wait
"#,
        )
        .unwrap();
        let lock = acquire_update_lock(dir.path()).unwrap();
        let path = dir.path().to_owned();
        let task =
            tokio::spawn(async move { install_release(&script, "v1.17.0", &path, lock).await });
        let pid_file = dir.path().join("installer.pid");
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(pid) = fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|text| text.trim().parse::<i32>().ok())
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(acquire_update_lock(dir.path()).is_err());
        task.abort();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap_err()
                .is_cancelled()
        );
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
        let _next_owner = acquire_update_lock(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(".labby-install/activation-journal/state")).unwrap(),
            "staged"
        );
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!dir.path().join("late-write").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_owner_child() {
        let Some(path) = std::env::var_os("LABBY_TEST_UPDATE_SIGNAL") else {
            return;
        };
        let path = Path::new(&path);
        let lock = acquire_update_lock(path).unwrap();
        if std::env::var_os("LABBY_TEST_LEGACY_UPDATE_OWNER").is_some() {
            // Reproduce the reviewed implementation: the parent alone owns
            // the lock while an ordinary subprocess performs installation.
            let _lock = lock;
            assert!(
                installer_command(&path.join("install.sh"), "v1.17.0", path)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        } else {
            install_release(&path.join("install.sh"), "v1.17.0", path, lock)
                .await
                .unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_keeps_exclusion_after_updater_is_killed() {
        use nix::sys::signal::{Signal, kill, killpg};
        use nix::unistd::Pid;
        use std::os::unix::process::CommandExt;
        struct GroupCleanup(Pid);
        impl Drop for GroupCleanup {
            fn drop(&mut self) {
                let _ = killpg(self.0, Signal::SIGKILL);
            }
        }
        for legacy in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("install.sh"),
                r#"
echo $$ > "$LABBY_INSTALL_DIR/installer.pid"
sleep 120
"#,
            )
            .unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "self_update::tests::installer_owner_child"])
                .env("LABBY_TEST_UPDATE_SIGNAL", dir.path())
                .process_group(0);
            if legacy {
                command.env("LABBY_TEST_LEGACY_UPDATE_OWNER", "1");
            }
            let mut updater = command.spawn().unwrap();
            let updater_pid = Pid::from_raw(i32::try_from(updater.id()).unwrap());
            let _updater_cleanup = GroupCleanup(updater_pid);
            let pid = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(pid) = fs::read_to_string(dir.path().join("installer.pid"))
                        .ok()
                        .and_then(|text| text.trim().parse::<i32>().ok())
                    {
                        break Pid::from_raw(pid);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let installer_group = if legacy { updater_pid } else { pid };
            let _installer_cleanup = GroupCleanup(installer_group);
            kill(updater_pid, Signal::SIGKILL).unwrap();
            assert!(!updater.wait().unwrap().success());
            // Baseline releases exclusion while the orphan is alive; the fixed
            // child-local descriptor keeps a replacement updater excluded.
            assert_eq!(acquire_update_lock(dir.path()).is_err(), !legacy);
            killpg(installer_group, Signal::SIGKILL).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if acquire_update_lock(dir.path()).is_ok() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[test]
    fn renamed_executable_cannot_report_an_update_to_another_binary() {
        assert_eq!(
            install_directory(Path::new("/opt/bin/labby")).unwrap(),
            Path::new("/opt/bin")
        );
        assert!(install_directory(Path::new("/opt/bin/labby-preview")).is_err());
    }

    #[test]
    fn failed_schedule_activation_restores_previous_job() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("update.plist");
        fs::write(&plist, "previous schedule").unwrap();
        let mut calls = Vec::new();
        let result = replace_schedule(&plist, b"new schedule", true, |operation| {
            calls.push(operation.to_owned());
            if calls.len() == 2 {
                assert_eq!(fs::read_to_string(&plist).unwrap(), "new schedule");
                bail!("bootstrap rejected");
            }
            if calls.len() == 3 {
                assert_eq!(fs::read_to_string(&plist).unwrap(), "previous schedule");
            }
            Ok(())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("previous schedule restored")
        );
        assert_eq!(calls, ["bootout", "bootstrap", "bootstrap"]);
        assert_eq!(fs::read_to_string(plist).unwrap(), "previous schedule");
    }

    #[test]
    fn failed_first_schedule_activation_removes_new_plist() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("update.plist");
        let result = replace_schedule(&plist, b"new schedule", false, |operation| {
            assert_eq!(operation, "bootstrap");
            bail!("bootstrap rejected")
        });
        assert!(result.is_err());
        assert!(!plist.exists());
    }

    #[test]
    fn missing_loaded_schedule_is_preserved_without_unloading() {
        let dir = tempfile::tempdir().unwrap();
        let result = replace_schedule(&dir.path().join("missing.plist"), b"new", true, |_| {
            panic!("must not unload a job without rollback data")
        });
        assert!(result.is_err());
    }

    #[test]
    fn schedule_rollback_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let plist = dir.path().join("update.plist");
        fs::write(&plist, "previous schedule").unwrap();
        let result = replace_schedule(&plist, b"new schedule", true, |operation| {
            if operation == "bootstrap" {
                bail!("bootstrap rejected");
            }
            Ok(())
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("restoration also failed")
        );
        assert_eq!(fs::read_to_string(plist).unwrap(), "previous schedule");
    }

    fn release(tag: &str) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: vec![
                Asset { name: ASSET.into() },
                Asset {
                    name: format!("{ASSET}.sha256"),
                },
            ],
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn server_restarts_only_after_successful_installation() {
        let mut results = std::collections::VecDeque::from([
            Err(anyhow::anyhow!("network failure")),
            Ok(json!({"installed": false})),
            Ok(json!({"installed": true, "version": "v1.17.0"})),
        ]);
        wait_for_installed_update(
            || std::future::ready(results.pop_front().expect("unexpected extra check")),
            Duration::ZERO,
            Duration::ZERO,
        )
        .await;
        assert!(results.is_empty());
    }

    #[test]
    fn selects_newest_stable_binary_without_downgrading() {
        let mut draft = release("v9.0.0");
        draft.draft = true;
        let mut prerelease = release("v8.0.0");
        prerelease.prerelease = true;
        let mut missing = release("v7.0.0");
        missing.assets.pop();
        let releases = vec![
            draft,
            prerelease,
            missing,
            release("v2.0.0-rc.1"),
            release("v1.9.0"),
            release("v1.10.0"),
        ];
        assert_eq!(select_release(&releases, [1, 8, 0]), Some("v1.10.0"));
        assert_eq!(select_release(&releases, [1, 10, 0]), None);
        assert_eq!(select_release(&releases, [1, 16, 1]), None);
    }

    #[test]
    fn unknown_versions_fail_closed() {
        for invalid in ["labby dev", "1.2", "1.2.3-rc.1", "1.2.+3", "1.2.3.4"] {
            assert!(version(invalid).is_err());
        }
        assert_eq!(version("labby 1.16.1\n").unwrap(), [1, 16, 1]);
    }

    #[test]
    fn launchd_executes_native_binary_and_escapes_paths() {
        let plist = launch_agent(
            Path::new("/custom & bin/labby"),
            Path::new("/logs/<update>"),
            "/bin",
        )
        .unwrap();
        assert!(plist.contains("<string>/custom &amp; bin/labby</string>"));
        assert!(plist.contains("<string>update</string><string>--automatic</string>"));
        assert!(plist.contains("<integer>86400</integer>"));
        assert!(!plist.contains("python"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_failure_propagates_without_reporting_success() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("install.sh");
        fs::write(&script, "echo 'attestation rejected' >&2; exit 7\n").unwrap();
        let lock = acquire_update_lock(temp.path()).unwrap();
        let error = install_release(&script, "v1.17.0", temp.path(), lock)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attestation rejected"));
    }

    #[test]
    fn installer_pins_release_and_disables_source_fallback() {
        let command = installer_command(Path::new("/installer"), "v1.17.0", Path::new("/bin"));
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("LABBY_INSTALL_VERSION")],
            Some(std::ffi::OsStr::new("v1.17.0"))
        );
        assert_eq!(
            env[std::ffi::OsStr::new("LABBY_ALLOW_SOURCE_FALLBACK")],
            Some(std::ffi::OsStr::new("0"))
        );
    }
}
