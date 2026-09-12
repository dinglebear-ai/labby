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

fn require_macos() -> Result<()> {
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

fn install_release(script: &Path, tag: &str, directory: &Path) -> Result<()> {
    let output = installer_command(script, tag, directory).output()?;
    if !output.status.success() {
        bail!(
            "Verified release install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Check published releases and atomically install a newer verified host binary.
pub(crate) async fn automatic(binary: &Path, dry_run: bool) -> Result<Value> {
    require_macos()?;
    let directory = binary.parent().context("Binary has no parent directory")?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(directory.join(".labby-auto-update.lock"))?;
    lock.try_lock()
        .context("Another automatic update is running")?;
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
        install_release(&script, tag, directory)?;
    }
    Ok(json!({"installed": !dry_run, "version": tag, "binary": binary, "dry_run": dry_run}))
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
    // Render and validate the replacement before unloading an existing job.
    let log_dir = home.join("Library/Logs/Labby");
    let content = launch_agent(
        &binary,
        &log_dir.join("auto-update.log"),
        &std::env::var("PATH")?,
    )?;
    if action == "enable"
        && !Command::new("gh")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    {
        bail!("GitHub CLI (gh) is required to verify release attestations");
    }
    if loaded
        && !Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{LABEL}")])
            .status()?
            .success()
    {
        bail!("Failed to unload the existing automatic update job");
    }
    if action == "disable" {
        if plist.exists() {
            fs::remove_file(&plist)?;
        }
        return Ok(json!({"enabled": false}));
    }
    fs::create_dir_all(plist.parent().context("Missing LaunchAgents directory")?)?;
    fs::create_dir_all(log_dir)?;
    fs::write(&plist, content)?;
    if !Command::new("launchctl")
        .arg("bootstrap")
        .arg(&domain)
        .arg(&plist)
        .status()?
        .success()
    {
        bail!("Failed to load automatic updates from {}", plist.display());
    }
    Ok(json!({"enabled": true, "binary": binary, "interval_seconds": 86400, "plist": plist}))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[test]
    fn installer_failure_propagates_without_reporting_success() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("install.sh");
        fs::write(&script, "echo 'attestation rejected' >&2; exit 7\n").unwrap();
        let error = install_release(&script, "v1.17.0", temp.path()).unwrap_err();
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
