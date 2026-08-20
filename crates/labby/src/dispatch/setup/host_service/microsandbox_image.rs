//! Microsandbox image preflight for host-service install/restart.
//!
//! Older deployments allowed mutable image aliases such as debian. Modern Code
//! Mode requires an immutable OCI digest and runs with pulling disabled, so a
//! restart must never discover an incompatible image after the healthy service
//! has already been stopped.

use std::path::{Path, PathBuf};

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

const SERVICE_ENV_PATH: &str = "/home/labby/.labby/.env";
const SYSTEM_DROPIN_DIR: &str = "/etc/systemd/system/labby.service.d";
const BACKEND_ENV: &str = "LABBY_CODE_MODE_RUNNER_BACKEND";
const MSB_EXE_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_EXE";
const MSB_IMAGE_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_IMAGE";
const MICROSANDBOX_BACKEND: &str = "microsandbox";
const SERVICE_USER: &str = "labby";

#[derive(Debug, Clone, PartialEq, Eq)]
enum PersistentSource {
    EnvFile(PathBuf),
    DropIn(PathBuf),
}

pub(super) async fn prepare_before_restart() -> Result<(), ToolError> {
    let effective = effective_service_environment().await?;
    if effective.get(BACKEND_ENV).map(String::as_str) != Some(MICROSANDBOX_BACKEND) {
        return Ok(());
    }

    let executable = effective.get(MSB_EXE_ENV).ok_or_else(|| {
        preflight_error(format!(
            "{BACKEND_ENV}=microsandbox requires {MSB_EXE_ENV} before restarting labby.service"
        ))
    })?;
    let image = effective.get(MSB_IMAGE_ENV).ok_or_else(|| {
        preflight_error(format!(
            "{BACKEND_ENV}=microsandbox requires {MSB_IMAGE_ENV} before restarting labby.service"
        ))
    })?;
    let executable = Path::new(executable);
    if !executable.is_absolute() || !executable.is_file() {
        return Err(preflight_error(format!(
            "{MSB_EXE_ENV} points at {}, which is not an installed absolute file",
            executable.display()
        )));
    }

    let (image_name, digest) = match immutable_parts(image) {
        Some((name, digest)) => (name.to_string(), digest.to_string()),
        None => (
            image.to_string(),
            inspect_cached_digest(executable, image).await?,
        ),
    };
    let canonical = canonical_pinned_reference(&image_name, &digest)?;
    ensure_canonical_cached(executable, &canonical).await?;

    if image == &canonical {
        return Ok(());
    }

    let source = locate_persistent_source(MSB_IMAGE_ENV)?;
    let changed_dropin = rewrite_persistent_source(&source, MSB_IMAGE_ENV, &canonical).await?;
    if changed_dropin {
        super::run_systemctl(&["daemon-reload"]).await?;
    }

    let refreshed = effective_service_environment().await?;
    if refreshed.get(MSB_IMAGE_ENV).map(String::as_str) != Some(canonical.as_str()) {
        return Err(preflight_error(format!(
            "migrated {MSB_IMAGE_ENV}, but systemd still resolves a different value; refusing to restart labby.service"
        )));
    }

    tracing::info!(
        service = "setup",
        action = "microsandbox_image.preflight",
        image_digest = %digest,
        source = %source_path(&source).display(),
        "prepared immutable cached Microsandbox image before service restart"
    );
    Ok(())
}

async fn effective_service_environment()
-> Result<std::collections::BTreeMap<String, String>, ToolError> {
    let output = super::run_systemctl(&[
        "show",
        super::SERVICE_NAME,
        "--property=Environment",
        "--value",
        "--no-pager",
    ])
    .await?;
    let env_file = std::fs::read_to_string(SERVICE_ENV_PATH).ok();
    Ok(merge_environment_sources(
        env_file.as_deref(),
        &output.stdout,
    ))
}

fn merge_environment_sources(
    env_file: Option<&str>,
    systemd_environment: &str,
) -> std::collections::BTreeMap<String, String> {
    // systemd.exec(5): EnvironmentFile= assignments override Environment=.
    // Mirror the service manager rather than treating `systemctl show Environment`
    // as the final effective environment (it omits EnvironmentFile contents).
    let mut merged = parse_environment_property(systemd_environment);
    if let Some(env_file) = env_file {
        merged.extend(parse_env_file(env_file));
    }
    merged
}

fn parse_env_file(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((
                key.trim().to_string(),
                value
                    .trim()
                    .trim_matches('\"')
                    .trim_matches('\'')
                    .to_string(),
            ))
        })
        .collect()
}

fn parse_environment_property(text: &str) -> std::collections::BTreeMap<String, String> {
    text.split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches('"').trim_matches('\'');
            let (key, value) = token.split_once('=')?;
            Some((
                key.to_string(),
                value.trim_matches('"').trim_matches('\'').to_string(),
            ))
        })
        .collect()
}

fn immutable_parts(image: &str) -> Option<(&str, &str)> {
    let (name, digest) = image.rsplit_once('@')?;
    let hex = digest.strip_prefix("sha256:")?;
    (!name.is_empty() && hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then_some((name, digest))
}

async fn inspect_cached_digest(executable: &Path, image: &str) -> Result<String, ToolError> {
    validate_mutable_alias(image)?;
    let executable = path_text(executable)?;
    let output = run_as_service_user(&[executable, "image", "inspect", image])
        .await
        .map_err(|err| {
            preflight_error(format!(
                "cannot migrate mutable {MSB_IMAGE_ENV}={image}: the image is not inspectable in the {SERVICE_USER} Microsandbox cache: {}",
                err.user_message()
            ))
        })?;
    extract_full_digest(&output.stdout).ok_or_else(|| {
        preflight_error(format!(
            "cannot migrate mutable {MSB_IMAGE_ENV}={image}: msb image inspect did not report a full sha256 digest"
        ))
    })
}

fn extract_full_digest(text: &str) -> Option<String> {
    text.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '"')
        .find_map(|token| {
            let token =
                token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '(' | ')' | '[' | ']'));
            let hex = token.strip_prefix("sha256:")?;
            (hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
                .then(|| format!("sha256:{}", hex.to_ascii_lowercase()))
        })
}

fn validate_mutable_alias(image: &str) -> Result<(), ToolError> {
    let invalid = image.trim().is_empty()
        || image.contains('@')
        || image.contains("://")
        || image.contains('?')
        || image.contains('#')
        || image.chars().any(char::is_whitespace)
        || image.starts_with('-')
        || image.starts_with('/');
    if invalid {
        return Err(preflight_error(format!(
            "cannot safely migrate legacy {MSB_IMAGE_ENV}={image}; set an immutable name@sha256:<64 hex> reference before restarting"
        )));
    }
    Ok(())
}

fn canonical_pinned_reference(image_name: &str, digest: &str) -> Result<String, ToolError> {
    let digest_hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| preflight_error("Microsandbox image digest must use sha256".to_string()))?;
    if digest_hex.len() != 64 || !digest_hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(preflight_error(
            "Microsandbox image digest must contain 64 hex characters".to_string(),
        ));
    }
    let name = canonical_image_name(image_name)?;
    Ok(format!("{name}@sha256:{}", digest_hex.to_ascii_lowercase()))
}

fn canonical_image_name(image: &str) -> Result<String, ToolError> {
    validate_mutable_alias(image)?;
    if let Some((first, _)) = image.split_once('/') {
        if first.contains('.') || first.contains(':') || first == "localhost" {
            return Ok(image.to_string());
        }
        return Ok(format!("docker.io/{image}"));
    }
    Ok(format!("docker.io/library/{image}"))
}

async fn ensure_canonical_cached(executable: &Path, canonical: &str) -> Result<(), ToolError> {
    let executable = path_text(executable)?;
    if run_as_service_user(&[executable, "image", "inspect", canonical])
        .await
        .is_ok()
    {
        return Ok(());
    }
    run_as_service_user(&[executable, "image", "pull", canonical])
        .await
        .map_err(|err| {
            preflight_error(format!(
                "failed to register immutable Microsandbox image {canonical} in the {SERVICE_USER} cache before restart: {}",
                err.user_message()
            ))
        })?;
    run_as_service_user(&[executable, "image", "inspect", canonical])
        .await
        .map_err(|err| {
            preflight_error(format!(
                "Microsandbox image pull completed but {canonical} is still not inspectable in the {SERVICE_USER} cache: {}",
                err.user_message()
            ))
        })?;
    Ok(())
}

async fn run_as_service_user(args: &[&str]) -> Result<super::CommandCapture, ToolError> {
    let mut runuser_args = vec!["-u", SERVICE_USER, "--", "env", "HOME=/home/labby"];
    runuser_args.extend_from_slice(args);
    super::run_command("runuser", &runuser_args).await
}

fn locate_persistent_source(key: &str) -> Result<PersistentSource, ToolError> {
    let env_path = PathBuf::from(SERVICE_ENV_PATH);
    // EnvironmentFile= overrides Environment= in systemd. If the service env
    // file defines the key, it is the winning persistent source even when a
    // drop-in also contains an Environment= assignment.
    if std::fs::read_to_string(&env_path)
        .ok()
        .is_some_and(|text| env_assignment_value(&text, key).is_some())
    {
        return Ok(PersistentSource::EnvFile(env_path));
    }

    let mut selected = None;
    let mut dropins = std::fs::read_dir(SYSTEM_DROPIN_DIR)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("conf"))
        .collect::<Vec<_>>();
    dropins.sort();
    for path in dropins {
        if std::fs::read_to_string(&path)
            .ok()
            .is_some_and(|text| systemd_environment_value(&text, key).is_some())
        {
            selected = Some(PersistentSource::DropIn(path));
        }
    }

    selected.ok_or_else(|| {
        preflight_error(format!(
            "{MSB_IMAGE_ENV} is effective in systemd but no persistent assignment was found in {SERVICE_ENV_PATH} or {SYSTEM_DROPIN_DIR}; refusing to restart"
        ))
    })
}

async fn rewrite_persistent_source(
    source: &PersistentSource,
    key: &str,
    value: &str,
) -> Result<bool, ToolError> {
    match source {
        PersistentSource::EnvFile(path) => {
            let outcome = env_merge::merge(
                path,
                MergeRequest {
                    entries: vec![EnvEntry::new(key, value).force()],
                    force: true,
                    expected_mtime: env_merge::snapshot_mtime(path),
                },
            )
            .map_err(|err| {
                preflight_error(format!("failed to update {}: {err}", path.display()))
            })?;
            if outcome.written > 0 {
                super::run_command("chown", &["labby:labby", path_text(path)?]).await?;
                if let Some(backup) = outcome.backup_path.as_deref() {
                    super::run_command("chown", &["labby:labby", path_text(backup)?]).await?;
                }
            }
            Ok(false)
        }
        PersistentSource::DropIn(path) => {
            let original = std::fs::read_to_string(path).map_err(super::io_error)?;
            let rewritten = rewrite_systemd_environment(&original, key, value)?;
            if rewritten == original {
                return Ok(false);
            }
            atomic_rewrite_preserving_metadata(path, rewritten.as_bytes())?;
            Ok(true)
        }
    }
}

fn rewrite_systemd_environment(text: &str, key: &str, value: &str) -> Result<String, ToolError> {
    let mut found = false;
    let mut out = String::with_capacity(text.len() + value.len());
    for line in text.split_inclusive('\n') {
        let had_newline = line.ends_with('\n');
        let body = line.strip_suffix('\n').unwrap_or(line);
        if systemd_environment_value(body, key).is_some() {
            found = true;
            out.push_str(&format!("Environment={key}={value}"));
            if had_newline {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    if !found {
        return Err(preflight_error(format!(
            "persistent source no longer contains {key}; refusing a stale migration write"
        )));
    }
    Ok(out)
}

fn env_assignment_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().trim_matches('"').trim_matches('\''))
        })
        .next_back()
}

fn systemd_environment_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| {
            let raw = line.trim().strip_prefix("Environment=")?.trim();
            let raw = raw.trim_matches('"').trim_matches('\'');
            let (name, value) = raw.split_once('=')?;
            (name == key).then(|| value.trim_matches('"').trim_matches('\''))
        })
        .next_back()
}

fn atomic_rewrite_preserving_metadata(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    use std::io::Write as _;

    let parent = path.parent().ok_or_else(|| {
        preflight_error(format!(
            "cannot determine parent directory for {}",
            path.display()
        ))
    })?;
    let metadata = std::fs::metadata(path).map_err(super::io_error)?;
    let permissions = metadata.permissions();
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(super::io_error)?;
    temp.write_all(bytes).map_err(super::io_error)?;
    temp.as_file_mut().sync_all().map_err(super::io_error)?;
    temp.persist(path)
        .map_err(|err| super::io_error(err.error))?;
    std::fs::set_permissions(path, permissions).map_err(super::io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        nix::unistd::chown(
            path,
            Some(nix::unistd::Uid::from_raw(metadata.uid())),
            Some(nix::unistd::Gid::from_raw(metadata.gid())),
        )
        .map_err(|err| {
            preflight_error(format!(
                "failed to preserve ownership on {}: {err}",
                path.display()
            ))
        })?;
    }
    if let Ok(dir) = std::fs::File::open(parent) {
        drop(dir.sync_all());
    }
    Ok(())
}

fn source_path(source: &PersistentSource) -> &Path {
    match source {
        PersistentSource::EnvFile(path) | PersistentSource::DropIn(path) => path,
    }
}

fn path_text(path: &Path) -> Result<&str, ToolError> {
    path.to_str()
        .ok_or_else(|| preflight_error(format!("path is not valid UTF-8: {}", path.display())))
}

fn preflight_error(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".into(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:d8f17b92dc7ff10f9c1fdecab0ad21103d1d24aed823c3a0359e4f50adfab3eb";

    #[test]
    fn extracts_only_full_sha256_digest() {
        assert_eq!(
            extract_full_digest(&format!("Reference: debian\nDigest: {DIGEST}\n")).as_deref(),
            Some(DIGEST)
        );
        assert!(extract_full_digest("Digest: sha256:d8f17b92dc7f").is_none());
    }

    #[test]
    fn canonicalizes_docker_short_names_and_namespaces() {
        assert_eq!(
            canonical_pinned_reference("debian", DIGEST).unwrap(),
            format!("docker.io/library/debian@{DIGEST}")
        );
        assert_eq!(
            canonical_pinned_reference("team/image:stable", DIGEST).unwrap(),
            format!("docker.io/team/image:stable@{DIGEST}")
        );
        assert_eq!(
            canonical_pinned_reference("registry.example:5000/team/image", DIGEST).unwrap(),
            format!("registry.example:5000/team/image@{DIGEST}")
        );
    }

    #[test]
    fn rejects_unsafe_mutable_aliases() {
        for value in [
            "",
            "-q",
            "https://example/image",
            "image latest",
            "/image",
            "image@sha256:bad",
        ] {
            assert!(validate_mutable_alias(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn parses_effective_systemd_environment() {
        let env = parse_environment_property(
            "HOME=/home/labby LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox LABBY_CODE_MODE_MICROSANDBOX_IMAGE=debian",
        );
        assert_eq!(
            env.get(BACKEND_ENV).map(String::as_str),
            Some("microsandbox")
        );
        assert_eq!(env.get(MSB_IMAGE_ENV).map(String::as_str), Some("debian"));
    }

    #[test]
    fn environment_file_values_override_systemd_environment_assignments() {
        let env_file = format!("{BACKEND_ENV}=microsandbox\n{MSB_IMAGE_ENV}=debian\n");
        let systemd = format!("{BACKEND_ENV}=process");
        let merged = merge_environment_sources(Some(&env_file), &systemd);

        assert_eq!(
            merged.get(BACKEND_ENV).map(String::as_str),
            Some("microsandbox")
        );
        assert_eq!(
            merged.get(MSB_IMAGE_ENV).map(String::as_str),
            Some("debian")
        );
    }

    #[test]
    fn rewrites_only_the_target_systemd_assignment() {
        let input = "[Service]\nEnvironment=LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox\nEnvironment=LABBY_CODE_MODE_MICROSANDBOX_IMAGE=debian\n";
        let pinned = format!("docker.io/library/debian@{DIGEST}");
        let output = rewrite_systemd_environment(input, MSB_IMAGE_ENV, &pinned).unwrap();
        assert!(output.contains("Environment=LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox"));
        assert!(output.contains(&format!("Environment={MSB_IMAGE_ENV}={pinned}")));
        assert!(!output.contains("MICROSANDBOX_IMAGE=debian\n"));
    }

    #[test]
    fn stale_source_rewrite_fails_closed() {
        let err = rewrite_systemd_environment("[Service]\n", MSB_IMAGE_ENV, "x").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("stale migration write"));
    }
}
