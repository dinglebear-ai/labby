//! Host executable updates and opt-in macOS scheduling.

use std::{fs, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const LABEL: &str = "net.labby.auto-update";
const ASSET: &str = "lab-aarch64-apple-darwin.tar.gz";
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

/// Release stream explicitly selected by an operator; never read from installer env.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Dev,
}

impl UpdateChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Dev => "dev",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    base: [u64; 3],
    // Stable sorts after every dev revision at the same base.
    revision: Option<u64>,
}

fn version(text: &str) -> Result<Version> {
    let text = text.trim().strip_prefix("labby ").unwrap_or(text.trim());
    let text = text.strip_prefix('v').unwrap_or(text);
    let (base, dev) = match text.split_once("-dev.") {
        Some((base, revision)) => (base, Some(revision)),
        None => (text, None),
    };
    let number = |part: &str| -> Result<u64> {
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            bail!("Expected a canonical Labby version, got {text:?}");
        }
        Ok(part.parse()?)
    };
    let parts = base.split('.').map(number).collect::<Result<Vec<_>>>()?;
    if parts.len() != 3 {
        bail!("Expected a Labby version, got {text:?}");
    }
    let dev = dev.map(number).transpose()?;
    if dev == Some(0) {
        bail!("Dev revision must be positive");
    }
    Ok(Version {
        base: [parts[0], parts[1], parts[2]],
        revision: dev,
    })
}

fn newer(candidate: Version, current: Version) -> bool {
    candidate.base > current.base
        || (candidate.base == current.base
            && match (candidate.revision, current.revision) {
                (None, Some(_)) => true,
                (Some(a), Some(b)) => a > b,
                _ => false,
            })
}

pub(crate) fn validate_requested_release(tag: &str, channel: UpdateChannel) -> Result<()> {
    if tag == "latest" {
        return Ok(());
    }
    if !tag.starts_with('v') || tag.trim() != tag {
        bail!("Release tag must be canonical vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-dev.N");
    }
    let parsed = version(tag)?;
    if parsed.revision.is_some() != (channel == UpdateChannel::Dev) {
        bail!(
            "Release {tag:?} does not belong to the {} channel",
            channel.as_str()
        );
    }
    Ok(())
}

fn selected_channel(directory: &Path) -> Result<UpdateChannel> {
    match fs::read_to_string(directory.join(".labby-update-channel")) {
        Ok(value) => match value.trim() {
            "stable" => Ok(UpdateChannel::Stable),
            "dev" => Ok(UpdateChannel::Dev),
            _ => bail!("Invalid persisted Labby update channel"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UpdateChannel::Stable),
        Err(error) => Err(error.into()),
    }
}

fn persist_channel(directory: &Path, channel: UpdateChannel) -> Result<()> {
    use std::io::Write;
    let mut stage = tempfile::NamedTempFile::new_in(directory)?;
    stage.write_all(channel.as_str().as_bytes())?;
    stage.as_file().sync_all()?;
    stage.persist(directory.join(".labby-update-channel"))?;
    Ok(())
}

fn select_release(releases: &[Release], current: Version, channel: UpdateChannel) -> Option<&str> {
    releases
        .iter()
        .filter(|r| !r.draft && r.prerelease == (channel == UpdateChannel::Dev))
        .filter(|r| validate_requested_release(&r.tag_name, channel).is_ok())
        .filter(|r| {
            [ASSET.to_string(), format!("{ASSET}.sha256")]
                .iter()
                .all(|name| r.assets.iter().any(|a| &a.name == name))
        })
        .filter_map(|r| {
            version(&r.tag_name)
                .ok()
                .filter(|v| {
                    v.revision.is_some() == (channel == UpdateChannel::Dev) && newer(*v, current)
                })
                .map(|v| (v, r.tag_name.as_str()))
        })
        .max_by(|(a, _), (b, _)| {
            if newer(*a, *b) {
                std::cmp::Ordering::Greater
            } else if newer(*b, *a) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .map(|(_, tag)| tag)
}

pub(crate) fn require_macos() -> Result<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        bail!("Automatic updates currently support macOS Apple Silicon only");
    }
    Ok(())
}

fn installer_command(
    script: &Path,
    tag: &str,
    directory: &Path,
    channel: UpdateChannel,
) -> Command {
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
        .env("LABBY_INSTALL_CHANNEL", channel.as_str())
        .env("LABBY_INSTALL_VERSION", tag)
        .env("LABBY_INSTALL_DIR", directory)
        .env("LABBY_INSTALL_REPO", REPO)
        .env("LABBY_INSTALL_NO_SETUP", "1")
        .env("LABBY_ALLOW_SOURCE_FALLBACK", "0");
    command
}

/// Run the same pinned, sanitized and mutually-exclusive installer used by
/// automatic updates for an operator-requested release.
pub(crate) async fn install_requested_release(
    tag: &str,
    directory: &Path,
    channel: UpdateChannel,
) -> Result<()> {
    validate_requested_release(tag, channel)?;
    fs::create_dir_all(directory)?;
    let lock = acquire_update_lock(directory)?;
    let temp = tempfile::tempdir()?;
    let script = temp.path().join("install.sh");
    fs::write(&script, INSTALL_SCRIPT)?;
    install_release(&script, tag, directory, lock.try_clone()?, channel).await?;
    Ok(())
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

async fn install_release(
    script: &Path,
    tag: &str,
    directory: &Path,
    lock: fs::File,
    channel: UpdateChannel,
) -> Result<()> {
    run_installer(installer_command(script, tag, directory, channel), lock).await
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

/// Uses the installer's existing mkdir/pid contract for standalone shell installs.
/// Never steal an existing lock: the installer owns stale
/// lock recovery, and an in-progress owner may not have published its PID yet.
struct InstallerTransaction {
    path: std::path::PathBuf,
}

impl Drop for InstallerTransaction {
    fn drop(&mut self) {
        if fs::read_to_string(self.path.join("pid")).ok().as_deref()
            == Some(&format!("{}\n", std::process::id()))
        {
            drop(fs::remove_file(self.path.join("pid")));
            drop(fs::remove_dir(&self.path));
        }
    }
}

fn acquire_installer_transaction(directory: &Path) -> Result<InstallerTransaction> {
    acquire_installer_transaction_with(directory, |path| {
        fs::write(path, format!("{}\n", std::process::id()))
    })
}

fn acquire_installer_transaction_with(
    directory: &Path,
    write_pid: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<InstallerTransaction> {
    let metadata = directory.join(".labby-install");
    fs::create_dir_all(&metadata)?;
    let path = metadata.join("transaction-lock");
    fs::create_dir(&path).context("Installer transaction is locked; finish the other installation or run installer recovery before retrying")?;
    let pid = path.join("pid");
    if let Err(error) = write_pid(&pid) {
        // Even a failed write may have created an empty or partial PID file.
        // Both paths are owned by this acquisition; preserve the original I/O
        // error while making a cleanup failure actionable.
        let cleanup = (|| -> std::io::Result<()> {
            match fs::remove_file(&pid) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::remove_dir(&path)
        })();
        if let Err(cleanup_error) = cleanup {
            return Err(error).with_context(|| {
                format!("Cannot publish installer lock PID; cleanup also failed: {cleanup_error}")
            });
        }
        return Err(error).context("Cannot publish installer lock PID; owned partial lock removed");
    }
    Ok(InstallerTransaction { path })
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
pub(crate) async fn automatic(
    binary: &Path,
    dry_run: bool,
    channel: Option<UpdateChannel>,
) -> Result<Value> {
    require_macos()?;
    automatic_with_catalog(binary, dry_run, channel, fetch_releases()).await
}

async fn fetch_releases() -> Result<Vec<Release>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("labby-auto-update")
        .build()?;
    fetch_release_catalog(
        &client,
        &format!("https://api.github.com/repos/{REPO}/releases"),
        10,
    )
    .await
}

async fn fetch_release_catalog(
    client: &reqwest::Client,
    endpoint: &str,
    max_pages: usize,
) -> Result<Vec<Release>> {
    tokio::time::timeout(Duration::from_secs(90), async {
        let mut releases = Vec::new();
        let mut total_bytes = 0;
        for page in 1..=max_pages {
            // Construct pages against the owned endpoint, never a URL supplied
            // by the Link header (which is only a continuation signal).
            let mut response = client
                .get(endpoint)
                .query(&[("per_page", 100), ("page", page)])
                .header("Accept", "application/vnd.github+json")
                .send()
                .await?
                .error_for_status()?;
            if !response.status().is_success() {
                bail!(
                    "Release catalog returned unexpected status {}",
                    response.status()
                );
            }
            let more = response
                .headers()
                .get_all(reqwest::header::LINK)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(','))
                .any(|link| {
                    link.split(';')
                        .skip(1)
                        .any(|part| part.trim() == "rel=\"next\"")
                });
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                total_bytes += chunk.len();
                if total_bytes > 4 * 1024 * 1024 {
                    bail!("Incomplete release discovery: catalog exceeds aggregate 4 MiB budget");
                }
                body.extend_from_slice(&chunk);
            }
            releases.extend(serde_json::from_slice::<Vec<Release>>(&body)?);
            if !more {
                return Ok(releases);
            }
        }
        bail!("Incomplete release discovery: more pages remain after the {max_pages}-page budget")
    })
    .await
    .context("Incomplete release discovery: catalog exceeded its 90-second deadline")?
}

async fn automatic_with_catalog(
    binary: &Path,
    dry_run: bool,
    channel: Option<UpdateChannel>,
    catalog: impl Future<Output = Result<Vec<Release>>>,
) -> Result<Value> {
    let directory = install_directory(binary)?;
    let lock = acquire_update_lock(directory)?;
    let requested_channel = channel;
    let channel = channel
        .map(Ok)
        .unwrap_or_else(|| selected_channel(directory))?;
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
        let mut command = installer_command(&script, "latest", directory, channel);
        command.env("LABBY_INSTALL_RECOVER_ONLY", "1");
        run_installer(command, lock.try_clone()?).await?;
    }
    let releases = catalog.await?;
    // A direct installer can run while the catalog is fetched. Observe its
    // resulting binary and marker together under the same transaction lock.
    let transaction = acquire_installer_transaction(directory)?;
    if journal.try_exists()? {
        bail!(
            "An installation was interrupted during release discovery; recover it before retrying"
        );
    }
    let observed_channel = selected_channel(directory)?;
    let channel = requested_channel.unwrap_or(observed_channel);
    let output = Command::new(binary).arg("--version").output()?;
    if !output.status.success() {
        bail!("Cannot read installed Labby version");
    }
    let current_text = std::str::from_utf8(&output.stdout)?.trim();
    let current = version(current_text)?;
    let Some(tag) = select_release(&releases, current, channel) else {
        if !dry_run && requested_channel.is_some() {
            persist_channel(directory, channel)?;
        }
        return Ok(
            json!({"installed": false, "channel": channel, "reason": format!("No newer {} Labby binary release", channel.as_str())}),
        );
    };
    // The child acquires this same lock itself. Its preconditions close the
    // release-before-spawn gap without inheriting or bypassing that lock.
    drop(transaction);
    if !dry_run {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("install.sh");
        fs::write(&script, INSTALL_SCRIPT)?;
        let mut command = installer_command(&script, tag, directory, channel);
        command
            .env("LABBY_INSTALL_EXPECTED_VERSION", current_text)
            .env("LABBY_INSTALL_EXPECTED_CHANNEL", observed_channel.as_str());
        run_installer(command, lock.try_clone()?).await?;
    }
    Ok(
        json!({"installed": !dry_run, "channel": channel, "version": tag, "binary": binary, "dry_run": dry_run}),
    )
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
        || automatic(&binary, false, None),
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
            Ok(_) => tracing::info!("automatic update check: no newer release"),
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
<key>ProgramArguments</key><array><string>{binary}</string><string>host</string><string>update</string><string>--automatic</string></array>
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
pub(crate) fn schedule(
    action: &str,
    dry_run: bool,
    channel: Option<UpdateChannel>,
) -> Result<Value> {
    require_macos()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    schedule_for_paths(
        action,
        dry_run,
        Path::new(&home),
        &std::env::current_exe()?,
        channel,
    )
}

fn schedule_for_paths(
    action: &str,
    dry_run: bool,
    home: &Path,
    binary: &Path,
    channel: Option<UpdateChannel>,
) -> Result<Value> {
    let requested_channel = channel;
    let channel = channel
        .map(Ok)
        .unwrap_or_else(|| selected_channel(install_directory(binary)?))?;
    let plist = home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"));
    if dry_run {
        return Ok(
            json!({"action": action, "channel": channel, "binary": binary, "plist": plist, "dry_run": true}),
        );
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
    // The embedded verified installer bootstraps its pinned verifier when the
    // scheduled update runs, so enabling updates needs no developer CLI.
    fs::create_dir_all(plist.parent().context("Missing LaunchAgents directory")?)?;
    fs::create_dir_all(log_dir)?;
    let channel = replace_schedule_channel(
        binary,
        &plist,
        content.as_bytes(),
        requested_channel,
        loaded,
        control,
    )?;
    Ok(
        json!({"enabled": true, "channel": channel, "binary": binary, "interval_seconds": 86400, "plist": plist}),
    )
}

/// The installer, service, and native schedule share one persisted selection.
/// Resolve it under the installer lock so enabling a schedule cannot overwrite
/// a concurrent manual installation's selection with an earlier observation.
fn replace_schedule_channel(
    binary: &Path,
    plist: &Path,
    content: &[u8],
    requested_channel: Option<UpdateChannel>,
    loaded: bool,
    control: impl FnMut(&str) -> Result<()>,
) -> Result<UpdateChannel> {
    let directory = install_directory(binary)?;
    let _update_lock = acquire_update_lock(directory)?;
    let _transaction = acquire_installer_transaction(directory)?;
    if directory
        .join(".labby-install/activation-journal")
        .try_exists()?
    {
        bail!("Recover the interrupted installation before changing the update schedule");
    }
    let marker = directory.join(".labby-update-channel");
    let previous_present = marker.try_exists()?;
    let previous_channel = selected_channel(directory)?;
    let channel = requested_channel.unwrap_or(previous_channel);
    persist_channel(directory, channel)?;
    if let Err(error) = replace_schedule(plist, content, loaded, control) {
        let restore = if previous_present {
            persist_channel(directory, previous_channel)
        } else {
            fs::remove_file(&marker).map_err(Into::into)
        };
        if let Err(restore_error) = restore {
            bail!(
                "Schedule activation failed: {error:#}; previous channel restoration failed: {restore_error:#}"
            );
        }
        return Err(error);
    }
    Ok(channel)
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
