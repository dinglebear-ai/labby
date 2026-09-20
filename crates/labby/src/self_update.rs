//! Host executable updates and opt-in macOS scheduling.

use std::{fs, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const LABEL: &str = "net.labby.auto-update";
const ASSET: &str = "lab-aarch64-apple-darwin.tar.gz";
const PROVENANCE_BUNDLE: &str = "release-provenance.sigstore.json";
const REPO: &str = "dinglebear-ai/labby";
const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install.sh");
const INSTALL_CONTROL_VARIABLES: &[&str] = &[
    "LABBY_INSTALL_RECOVER_ONLY",
    "LABBY_INSTALL_ROLLBACK",
    "LABBY_INSTALL_LOCAL_BINARY",
    "LABBY_INSTALL_LOCAL_SHA256",
];
/// GitHub CLI overrides that would let the service environment select the
/// host, credential store, or enterprise token the installer verifies release
/// provenance against. Releases are attested on github.com only.
const GITHUB_HOST_OVERRIDES: &[&str] = &["GH_HOST", "GH_ENTERPRISE_TOKEN", "GH_CONFIG_DIR"];

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
            [
                ASSET.to_string(),
                format!("{ASSET}.sha256"),
                PROVENANCE_BUNDLE.to_string(),
            ]
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
    for key in INSTALL_CONTROL_VARIABLES
        .iter()
        .chain(GITHUB_HOST_OVERRIDES)
    {
        command.env_remove(key);
    }
    command
        .env("LABBY_INSTALL_VERSION", tag)
        .env("LABBY_INSTALL_DIR", directory)
        .env("LABBY_INSTALL_REPO", REPO)
        .env("LABBY_INSTALL_NO_SETUP", "1")
        .env("LABBY_ALLOW_SOURCE_FALLBACK", "0");
    command
}

/// Run the same pinned, sanitized and mutually-exclusive installer used by
/// automatic updates for an operator-requested release.
pub(crate) async fn install_requested_release(tag: &str, directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)?;
    let lock = acquire_update_lock(directory)?;
    let temp = tempfile::tempdir()?;
    let script = temp.path().join("install.sh");
    fs::write(&script, INSTALL_SCRIPT)?;
    install_release(&script, tag, directory, lock).await
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
    run_installer(installer_command(script, tag, directory), lock).await
}

async fn run_installer(mut command: Command, lock: fs::File) -> Result<()> {
    use std::io::{Read, Seek};
    let mut stderr = tempfile::tempfile()?;
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
    automatic_with_catalog(binary, dry_run, fetch_releases()).await
}

async fn fetch_releases() -> Result<Vec<Release>> {
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
    Ok(serde_json::from_slice(&body)?)
}

async fn automatic_with_catalog(
    binary: &Path,
    dry_run: bool,
    catalog: impl Future<Output = Result<Vec<Release>>>,
) -> Result<Value> {
    let directory = install_directory(binary)?;
    let lock = acquire_update_lock(directory)?;
    let journal = directory.join(".labby-install/activation-journal");
    if journal.try_exists()? {
        if dry_run {
            return Ok(
                json!({"installed": false, "dry_run": true, "recovery_required": true,
                "reason": "Interrupted installation must be recovered before checking for updates"}),
            );
        }
        // Recovery owns the transaction format. Run it before executing the
        // published binary or polling the release catalog, including offline.
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("install.sh");
        fs::write(&script, INSTALL_SCRIPT)?;
        let mut command = installer_command(&script, "latest", directory);
        command.env("LABBY_INSTALL_RECOVER_ONLY", "1");
        run_installer(command, lock.try_clone()?).await?;
    }
    let output = Command::new(binary).arg("--version").output()?;
    if !output.status.success() {
        bail!("Cannot read installed Labby version");
    }
    let current = version(std::str::from_utf8(&output.stdout)?)?;
    let releases = catalog.await?;
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
pub(crate) async fn wait_for_installed_update<F, Fut>(
    mut check: F,
    initial: Duration,
    interval: Duration,
) where
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

struct ScheduleLock(fs::File);

impl Drop for ScheduleLock {
    fn drop(&mut self) {
        drop(self.0.unlock());
    }
}

fn acquire_schedule_lock(plist: &Path) -> Result<ScheduleLock> {
    fs::create_dir_all(plist.parent().context("Missing LaunchAgents directory")?)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(plist.with_extension("lock"))?;
    lock.try_lock()
        .context("Another updater schedule operation is running")?;
    Ok(ScheduleLock(lock))
}

/// Configure or inspect the per-user launchd job, which invokes this executable.
pub(crate) fn schedule(action: &str, dry_run: bool) -> Result<Value> {
    require_macos()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    schedule_for_paths(action, dry_run, Path::new(&home), &std::env::current_exe()?)
}

fn schedule_for_paths(action: &str, dry_run: bool, home: &Path, binary: &Path) -> Result<Value> {
    let plist = home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"));
    if dry_run {
        return Ok(json!({"action": action, "binary": binary, "plist": plist, "dry_run": true}));
    }
    let _schedule_lock = if action == "status" {
        None
    } else {
        Some(acquire_schedule_lock(&plist)?)
    };
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
        binary,
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
mod tests;
