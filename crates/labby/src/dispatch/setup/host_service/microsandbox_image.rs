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
    let migration = async {
        let changed_dropin =
            rewrite_persistent_source(&source, MSB_IMAGE_ENV, &canonical).await?;
        if changed_dropin {
            super::run_systemctl(&["daemon-reload"]).await?;
        }

        let refreshed = effective_service_environment().await?;
        if refreshed.get(MSB_IMAGE_ENV).map(String::as_str) != Some(canonical.as_str()) {
            return Err(preflight_error(format!(
                "migrated {MSB_IMAGE_ENV}, but systemd still resolves a different value; refusing to restart labby.service"
            )));
        }
        Ok(())
    }
    .await;
    if let Err(error) = migration {
        if let Err(rollback_error) =
            restore_persistent_source(&source, MSB_IMAGE_ENV, image, &canonical).await
        {
            return Err(preflight_error(format!(
                "{}; rollback also failed: {}",
                error.user_message(),
                rollback_error.user_message()
            )));
        }
        return Err(error);
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

async fn restore_persistent_source(
    source: &PersistentSource,
    key: &str,
    original_value: &str,
    migrated_value: &str,
) -> Result<(), ToolError> {
    match source {
        PersistentSource::EnvFile(path) => {
            let before = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            let text = std::fs::read_to_string(path).map_err(super::io_error)?;
            let after = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            if before != after {
                return Err(preflight_error(format!(
                    "persistent {key} changed concurrently; refusing rollback"
                )));
            }
            let current = env_assignment_value(&text, key)?;
            ensure_rollback_value(key, current.as_deref(), original_value, migrated_value)?;
            if current.as_deref() == Some(original_value) {
                return Ok(());
            }
            env_merge::merge(
                path,
                MergeRequest {
                    entries: vec![EnvEntry::new(key, original_value).force()],
                    force: true,
                    expected_mtime: Some(after),
                },
            )
            .map_err(|error| {
                preflight_error(format!("failed to roll back {}: {error}", path.display()))
            })?;
        }
        PersistentSource::DropIn(path) => {
            let lock_path = PathBuf::from(format!("{}.lock", path.display()));
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .map_err(super::io_error)?;
            lock.lock().map_err(super::io_error)?;
            let before = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            let text = std::fs::read_to_string(path).map_err(super::io_error)?;
            let after = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            if before != after {
                return Err(preflight_error(format!(
                    "persistent {key} changed concurrently; refusing rollback"
                )));
            }
            let current = systemd_environment_value(&text, key)?;
            ensure_rollback_value(key, current.as_deref(), original_value, migrated_value)?;
            if current.as_deref() == Some(original_value) {
                return Ok(());
            }
            let rewritten = rewrite_systemd_environment(&text, key, original_value)?;
            atomic_rewrite_preserving_metadata(path, rewritten.as_bytes(), Some(after))?;
            super::run_systemctl(&["daemon-reload"]).await?;
        }
    }
    Ok(())
}

fn ensure_rollback_value(
    key: &str,
    current: Option<&str>,
    original_value: &str,
    migrated_value: &str,
) -> Result<(), ToolError> {
    if current == Some(original_value) {
        return Ok(());
    }
    if current != Some(migrated_value) {
        return Err(preflight_error(format!(
            "persistent {key} changed concurrently; refusing to overwrite it during rollback"
        )));
    }
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
    let env_file = match std::fs::read_to_string(SERVICE_ENV_PATH) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(preflight_error(format!(
                "cannot read service EnvironmentFile {SERVICE_ENV_PATH}: {error}"
            )));
        }
    };
    merge_environment_sources(env_file.as_deref(), &output.stdout)
}

fn merge_environment_sources(
    env_file: Option<&str>,
    systemd_environment: &str,
) -> Result<std::collections::BTreeMap<String, String>, ToolError> {
    // systemd.exec(5): EnvironmentFile= assignments override Environment=.
    // Mirror the service manager rather than treating `systemctl show Environment`
    // as the final effective environment (it omits EnvironmentFile contents).
    let mut merged = parse_environment_property(systemd_environment)?;
    if let Some(env_file) = env_file {
        merged.extend(parse_env_file(env_file)?);
    }
    Ok(merged)
}

fn parse_env_file(text: &str) -> Result<std::collections::BTreeMap<String, String>, ToolError> {
    let mut parsed = std::collections::BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let relevant = matches!(key, BACKEND_ENV | MSB_EXE_ENV | MSB_IMAGE_ENV);
        if !relevant {
            if key
                .strip_prefix("export ")
                .is_some_and(|key| matches!(key, BACKEND_ENV | MSB_EXE_ENV | MSB_IMAGE_ENV))
            {
                return Err(preflight_error(format!(
                    "invalid EnvironmentFile variable name at line {}: {key:?}",
                    index + 1
                )));
            }
            continue;
        }
        let value = parse_systemd_environment_file_value(value.trim()).map_err(|error| {
            preflight_error(format!(
                "invalid EnvironmentFile value at line {}: {}",
                index + 1,
                error.user_message()
            ))
        })?;
        parsed.insert(key.to_string(), value);
    }
    Ok(parsed)
}

fn parse_systemd_environment_file_value(text: &str) -> Result<String, ToolError> {
    let mut value = String::new();
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && quote != Some('\'') {
            match (quote, chars.peek().copied()) {
                (Some('"'), Some(next)) if !matches!(next, '"' | '\\' | '$' | '`' | '\n') => {
                    value.push('\\');
                }
                (_, Some('\n')) => {
                    chars.next();
                }
                (_, Some(_)) => value.push(chars.next().expect("peeked character exists")),
                (_, None) => return Err(preflight_error("trailing escape".to_string())),
            }
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else {
            value.push(ch);
        }
    }
    if quote.is_some() {
        return Err(preflight_error("unterminated quote".to_string()));
    }
    Ok(value)
}

fn parse_environment_property(
    text: &str,
) -> Result<std::collections::BTreeMap<String, String>, ToolError> {
    Ok(split_systemd_words(text)?
        .into_iter()
        .filter_map(|token| {
            let (key, value) = token.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect())
}

fn split_systemd_words(text: &str) -> Result<Vec<String>, ToolError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && quote != Some('\'') {
            match (quote, chars.peek().copied()) {
                (Some('"'), Some(next)) if !matches!(next, '"' | '\\' | '$' | '`' | '\n') => {
                    word.push('\\');
                }
                (_, Some('\n')) => {
                    chars.next();
                }
                (_, Some(_)) => word.push(chars.next().expect("peeked character exists")),
                (_, None) => {
                    return Err(preflight_error(
                        "invalid systemd environment assignment: trailing escape".to_string(),
                    ));
                }
            }
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if quote.is_some() {
        return Err(preflight_error(
            "invalid systemd environment assignment: unterminated quote".to_string(),
        ));
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
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
    match std::fs::read_to_string(&env_path) {
        Ok(text) if env_assignment_value(&text, key)?.is_some() => {
            return Ok(PersistentSource::EnvFile(env_path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(super::io_error(error)),
    }

    let mut selected = None;
    let mut dropins = match std::fs::read_dir(SYSTEM_DROPIN_DIR) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()).map_err(super::io_error))
            .collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(super::io_error(error)),
    };
    dropins.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("conf"));
    dropins.sort();
    for path in dropins {
        let text = std::fs::read_to_string(&path).map_err(super::io_error)?;
        if systemd_environment_value(&text, key)?.is_some() {
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
                let ownership_result = async {
                    if let Some(backup) = outcome.backup_path.as_deref() {
                        super::run_command("chown", &["labby:labby", path_text(backup)?]).await?;
                    }
                    super::run_command("chown", &["labby:labby", path_text(path)?]).await
                }
                .await;
                if let Err(error) = ownership_result {
                    if let Some(backup) = outcome.backup_path.as_deref() {
                        std::fs::rename(backup, path).map_err(super::io_error)?;
                        let parent = path.parent().ok_or_else(|| {
                            preflight_error(format!(
                                "cannot determine parent directory for {}",
                                path.display()
                            ))
                        })?;
                        std::fs::File::open(parent)
                            .and_then(|dir| dir.sync_all())
                            .map_err(super::io_error)?;
                    }
                    return Err(error);
                }
            }
            Ok(false)
        }
        PersistentSource::DropIn(path) => {
            let before = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            let original = std::fs::read_to_string(path).map_err(super::io_error)?;
            let after = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .map_err(super::io_error)?;
            if before != after {
                return Err(preflight_error(format!(
                    "{} changed concurrently; refusing migration",
                    path.display()
                )));
            }
            let rewritten = rewrite_systemd_environment(&original, key, value)?;
            if rewritten == original {
                return Ok(false);
            }
            atomic_rewrite_preserving_metadata(path, rewritten.as_bytes(), Some(after))?;
            Ok(true)
        }
    }
}

fn rewrite_systemd_environment(text: &str, key: &str, value: &str) -> Result<String, ToolError> {
    let mut found = false;
    let mut out = String::with_capacity(text.len() + value.len());
    let key_prefix = format!("{key}=");
    for line in text.split_inclusive('\n') {
        let had_newline = line.ends_with('\n');
        let body = line.strip_suffix('\n').unwrap_or(line);
        if let Some(assignments) = parse_systemd_environment_line(body)?
            && assignments
                .iter()
                .any(|assignment| assignment.starts_with(&key_prefix))
        {
            found = true;
            out.push_str("Environment=");
            for (index, assignment) in assignments.iter().enumerate() {
                if index > 0 {
                    out.push(' ');
                }
                let assignment = if assignment.starts_with(&key_prefix) {
                    format!("{key}={value}")
                } else {
                    assignment.clone()
                };
                out.push('"');
                for ch in assignment.chars() {
                    if matches!(ch, '\\' | '"') {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                out.push('"');
            }
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

fn env_assignment_value(text: &str, key: &str) -> Result<Option<String>, ToolError> {
    Ok(parse_env_file(text)?.remove(key))
}

fn parse_systemd_environment_line(line: &str) -> Result<Option<Vec<String>>, ToolError> {
    let Some(raw) = line.trim().strip_prefix("Environment=") else {
        return Ok(None);
    };
    split_systemd_words(raw.trim()).map(Some)
}

fn systemd_environment_value(text: &str, key: &str) -> Result<Option<String>, ToolError> {
    let mut found = None;
    for line in text.lines() {
        if let Some(assignments) = parse_systemd_environment_line(line)?
            && let Some(value) = assignments.into_iter().find_map(|assignment| {
                let (name, value) = assignment.split_once('=')?;
                (name == key).then(|| value.to_string())
            })
        {
            found = Some(value);
        }
    }
    Ok(found)
}

fn atomic_rewrite_preserving_metadata(
    path: &Path,
    bytes: &[u8],
    expected_mtime: Option<std::time::SystemTime>,
) -> Result<(), ToolError> {
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
    temp.as_file_mut()
        .set_permissions(permissions)
        .map_err(super::io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        nix::unistd::chown(
            temp.path(),
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
    temp.as_file_mut().sync_all().map_err(super::io_error)?;
    if let Some(expected) = expected_mtime {
        let current = std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map_err(super::io_error)?;
        if current != expected {
            return Err(preflight_error(format!(
                "{} changed concurrently; refusing atomic rewrite",
                path.display()
            )));
        }
    }
    temp.persist(path)
        .map_err(|err| super::io_error(err.error))?;
    let dir = std::fs::File::open(parent).map_err(super::io_error)?;
    dir.sync_all().map_err(super::io_error)?;
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
        )
        .unwrap();
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
        let merged = merge_environment_sources(Some(&env_file), &systemd).unwrap();

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
        assert!(output.contains(&format!("Environment=\"{MSB_IMAGE_ENV}={pinned}\"")));
        assert!(!output.contains("MICROSANDBOX_IMAGE=debian\n"));
    }

    #[test]
    fn rewrites_target_inside_multi_assignment_without_losing_siblings() {
        let input =
            format!("[Service]\nEnvironment=KEEP=hello\\ world \"{MSB_IMAGE_ENV}=debian\"\n");
        let pinned = format!("docker.io/library/debian@{DIGEST}");
        let output = rewrite_systemd_environment(&input, MSB_IMAGE_ENV, &pinned).unwrap();
        assert!(output.contains("\"KEEP=hello world\""));
        assert!(output.contains(&format!("\"{MSB_IMAGE_ENV}={pinned}\"")));
    }

    #[test]
    fn rewrite_preserves_effective_sibling_assignment_values() {
        let input = format!(
            "Environment=\"SPACE=hello world\" 'SLASH=C:\\\\tools' \"EQUAL=a=b\" \"{MSB_IMAGE_ENV}=debian\"\n"
        );
        let before = parse_systemd_environment_line(input.trim())
            .unwrap()
            .unwrap();
        let output = rewrite_systemd_environment(&input, MSB_IMAGE_ENV, "pinned").unwrap();
        let after = parse_systemd_environment_line(output.trim())
            .unwrap()
            .unwrap();

        assert_eq!(&before[..3], &after[..3]);
        assert_eq!(after[3], format!("{MSB_IMAGE_ENV}=pinned"));
    }

    #[test]
    fn rejects_shell_export_syntax_in_environment_file() {
        let text = format!("export {BACKEND_ENV}=microsandbox\n");
        assert!(parse_env_file(&text).is_err());
    }

    #[test]
    fn ignores_systemd_environment_file_comments_and_non_assignments() {
        let text = format!("; comment\nignored text\n{BACKEND_ENV}=microsandbox\n");
        let parsed = parse_env_file(&text).unwrap();
        assert_eq!(
            parsed.get(BACKEND_ENV).map(String::as_str),
            Some("microsandbox")
        );
    }

    #[test]
    fn preserves_unquoted_whitespace_in_environment_file_values() {
        let parsed = parse_env_file(&format!("{MSB_EXE_ENV}=path with spaces\n")).unwrap();
        assert_eq!(
            parsed.get(MSB_EXE_ENV).map(String::as_str),
            Some("path with spaces")
        );
    }

    #[test]
    fn rejects_malformed_systemd_assignments_before_rewrite() {
        for input in [
            format!("Environment=\"{MSB_IMAGE_ENV}=debian\n"),
            format!("Environment={MSB_IMAGE_ENV}=debian\\\n"),
        ] {
            let error = rewrite_systemd_environment(&input, MSB_IMAGE_ENV, "pinned").unwrap_err();
            assert_eq!(error.kind(), "invalid_param");
        }
    }

    #[test]
    fn rewrite_preserves_double_quoted_non_special_backslashes() {
        let input = format!("Environment=\"KEEP=C:\\tools\" \"{MSB_IMAGE_ENV}=debian\"\n");
        let output = rewrite_systemd_environment(&input, MSB_IMAGE_ENV, "pinned").unwrap();
        assert_eq!(
            systemd_environment_value(&output, "KEEP")
                .unwrap()
                .as_deref(),
            Some("C:\\tools")
        );
    }

    #[test]
    fn stale_source_rewrite_fails_closed() {
        let err = rewrite_systemd_environment("[Service]\n", MSB_IMAGE_ENV, "x").unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("stale migration write"));
    }
}
