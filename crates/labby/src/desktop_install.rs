//! Verified installation of the prebuilt Labby Tauri application.
//!
//! Desktop installation intentionally consumes release artifacts only. It never
//! invokes pnpm, Cargo/Tauri builds, or a platform package manager on the user's
//! behalf.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use serde_json::json;

const REPO: &str = "dinglebear-ai/labby";
const SIGNER_WORKFLOW: &str = "dinglebear-ai/labby/.github/workflows/release.yml";

pub(crate) fn install_release_app(control_plane_url: &str) -> Result<()> {
    let asset = platform_asset()?;
    ensure_command(
        "gh",
        "GitHub CLI (gh) is required to verify the desktop release artifact",
    )?;
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let temp = tempfile::tempdir().context("create desktop download directory")?;
    run_checked(
        "gh",
        &[
            "release",
            "download",
            &tag,
            "--repo",
            REPO,
            "--pattern",
            asset,
            "--dir",
            temp.path()
                .to_str()
                .context("desktop temp path is not UTF-8")?,
        ],
        "download Labby desktop release",
    )?;
    let artifact = temp.path().join(asset);
    if !artifact.is_file() {
        bail!("release {tag} does not contain the expected desktop asset {asset}");
    }
    run_checked(
        "gh",
        &[
            "attestation",
            "verify",
            artifact
                .to_str()
                .context("desktop artifact path is not UTF-8")?,
            "--repo",
            REPO,
            "--signer-workflow",
            SIGNER_WORKFLOW,
            "--source-ref",
            &format!("refs/tags/{tag}"),
            "--deny-self-hosted-runners",
        ],
        "verify Labby desktop release provenance",
    )?;

    install_artifact(&artifact)?;
    seed_control_plane(control_plane_url)?;
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn platform_asset() -> Result<&'static str> {
    Ok("labby-desktop-macos-arm64.tar.gz")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn platform_asset() -> Result<&'static str> {
    Ok("labby-desktop-linux-x86_64.AppImage")
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
fn platform_asset() -> Result<&'static str> {
    bail!("a prebuilt Labby desktop app is not published for this platform yet")
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn install_artifact(artifact: &Path) -> Result<()> {
    let home = dirs::home_dir().context("determine home directory for desktop installation")?;
    let applications = home.join("Applications");
    fs::create_dir_all(&applications)?;
    let stage = tempfile::tempdir_in(&applications)?;
    run_checked(
        "tar",
        &[
            "-xzf",
            artifact
                .to_str()
                .context("desktop artifact path is not UTF-8")?,
            "-C",
            stage
                .path()
                .to_str()
                .context("desktop stage path is not UTF-8")?,
        ],
        "extract Labby desktop app",
    )?;
    let source = stage.path().join("Labby.app");
    if !source.is_dir() {
        bail!("desktop archive did not contain Labby.app");
    }
    let destination = applications.join("Labby.app");
    let backup = applications.join(".Labby.app.previous");
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    if destination.exists() {
        fs::rename(&destination, &backup)?;
    }
    if let Err(error) = fs::rename(&source, &destination) {
        if backup.exists() {
            drop(fs::rename(&backup, &destination));
        }
        return Err(error).context("activate Labby desktop app");
    }
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn install_artifact(artifact: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let home = dirs::home_dir().context("determine home directory for desktop installation")?;
    let bin_dir = home.join(".local/bin");
    fs::create_dir_all(&bin_dir)?;
    let destination = bin_dir.join("labby-desktop");
    let stage = bin_dir.join(".labby-desktop.tmp");
    fs::copy(artifact, &stage)?;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o755))?;
    fs::rename(&stage, &destination)?;

    let applications = home.join(".local/share/applications");
    fs::create_dir_all(&applications)?;
    let desktop = applications.join("labby.desktop");
    let mut file = fs::File::create(&desktop)?;
    writeln!(
        file,
        "[Desktop Entry]\nType=Application\nName=Labby\nComment=Labby Control Plane\nExec={}\nTerminal=false\nCategories=Development;",
        destination.display()
    )?;
    Ok(())
}

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
fn install_artifact(_artifact: &Path) -> Result<()> {
    bail!("desktop installation is unavailable on this platform")
}

fn seed_control_plane(control_plane_url: &str) -> Result<()> {
    let url = url::Url::parse(control_plane_url).context("desktop Control Plane URL is invalid")?;
    let loopback = matches!(url.host(), Some(url::Host::Ipv4(value)) if value.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(value)) if value.is_loopback())
        || matches!(url.host(), Some(url::Host::Domain(value)) if value.eq_ignore_ascii_case("localhost"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("desktop Control Plane URL must use HTTPS except for loopback HTTP");
    }
    let settings = desktop_settings_path()?;
    if let Some(parent) = settings.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec_pretty(&json!({
        "controlPlaneUrl": control_plane_url.trim_end_matches('/'),
    }))?;
    let mut tmp = tempfile::NamedTempFile::new_in(settings.parent().unwrap_or(Path::new(".")))?;
    tmp.write_all(&payload)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(&settings)
        .map_err(|error| error.error)
        .with_context(|| format!("write desktop settings {}", settings.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn desktop_settings_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("determine home directory for desktop settings")?;
    Ok(home.join("Library/Application Support/tv.tootie.labby.desktop/settings.json"))
}

#[cfg(target_os = "linux")]
fn desktop_settings_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("determine home directory for desktop settings")?;
    Ok(home.join(".config/tv.tootie.labby.desktop/settings.json"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn desktop_settings_path() -> Result<PathBuf> {
    bail!("desktop settings path is unavailable on this platform")
}

fn ensure_command(program: &str, message: &str) -> Result<()> {
    let available = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        bail!("{message}");
    }
    Ok(())
}

fn run_checked(program: &str, args: &[&str], action: &str) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("{action}: launch {program}"))?;
    if !status.success() {
        bail!("{action} failed with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_settings_reject_remote_plaintext_http() {
        assert!(seed_control_plane("http://192.168.1.20:8765").is_err());
    }
}
