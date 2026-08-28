//! Resolve the process boundary used for the Javy/QuickJS runner.
//!
//! The default remains a direct self re-exec. Operators may opt into a
//! Microsandbox microVM; only the runner crosses that boundary. MCP discovery,
//! authorization, dispatch, and credentials remain in the Labby host process.

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

use crate::error::ToolError;
use crate::pool::{MicrosandboxSpawn, RunnerSpawn};

const BACKEND_ENV: &str = "LABBY_CODE_MODE_RUNNER_BACKEND";
#[cfg(target_os = "linux")]
const MSB_EXE_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_EXE";
#[cfg(target_os = "linux")]
const MSB_IMAGE_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_IMAGE";

pub(super) fn resolve_runner_spawn() -> Result<(RunnerSpawn, Option<MicrosandboxSpawn>), ToolError>
{
    match std::env::var(BACKEND_ENV).as_deref() {
        Ok("microsandbox") => microsandbox_spawn(),
        Ok("process") | Err(std::env::VarError::NotPresent) => process_spawn(),
        Ok(value) => Err(invalid_param(format!(
            "{BACKEND_ENV} must be `process` or `microsandbox`, got `{value}`"
        ))),
        Err(std::env::VarError::NotUnicode(_)) => Err(invalid_param(format!(
            "{BACKEND_ENV} must contain valid UTF-8"
        ))),
    }
}

fn process_spawn() -> Result<(RunnerSpawn, Option<MicrosandboxSpawn>), ToolError> {
    Ok((RunnerSpawn::try_default()?, None))
}

fn microsandbox_spawn() -> Result<(RunnerSpawn, Option<MicrosandboxSpawn>), ToolError> {
    #[cfg(not(target_os = "linux"))]
    return Err(invalid_param(
        "the Microsandbox Code Mode runner backend is supported only on Linux",
    ));

    #[cfg(target_os = "linux")]
    {
        let runner = RunnerSpawn::try_default()?;
        let msb = required_absolute_executable(MSB_EXE_ENV)?;
        let image = required_nonempty(MSB_IMAGE_ENV)?;
        let image_digest = image
            .rsplit_once('@')
            .map_or("sha256:<validated>", |(_, digest)| digest);
        tracing::info!(
            backend = "microsandbox",
            image_digest,
            runner = %runner.program.display(),
            msb = %msb.display(),
            network = "disabled",
            "using Microsandbox for Code Mode runner isolation"
        );

        Ok((
            runner,
            Some(MicrosandboxSpawn {
                executable: msb,
                image,
            }),
        ))
    }
}

#[cfg(target_os = "linux")]
fn required_nonempty(name: &str) -> Result<String, ToolError> {
    let value = std::env::var(name)
        .map_err(|_| invalid_param(format!("{name} is required for the Microsandbox backend")))?;
    validate_image_reference(name, &value)
}

#[cfg(target_os = "linux")]
fn validate_image_reference(name: &str, value: &str) -> Result<String, ToolError> {
    let value = value.trim();
    let Some((image_name, digest)) = value.split_once('@') else {
        return Err(invalid_param(format!(
            "{name} must be an immutable OCI reference in the form name@sha256:<64 hex>"
        )));
    };
    let digest_hex = digest.strip_prefix("sha256:").ok_or_else(|| {
        invalid_param(format!(
            "{name} must use an immutable sha256 digest in the form name@sha256:<64 hex>"
        ))
    })?;
    let invalid_name = image_name.is_empty()
        || image_name.starts_with('-')
        || image_name.starts_with('/')
        || image_name.contains('@')
        || image_name.contains("://")
        || image_name.contains('?')
        || image_name.contains('#')
        || image_name.chars().any(char::is_whitespace);
    if invalid_name || digest_hex.len() != 64 || !digest_hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(invalid_param(format!(
            "{name} must be an immutable OCI reference in the form name@sha256:<64 hex>"
        )));
    }
    Ok(format!(
        "{image_name}@sha256:{}",
        digest_hex.to_ascii_lowercase()
    ))
}

#[cfg(target_os = "linux")]
fn required_absolute_executable(name: &str) -> Result<PathBuf, ToolError> {
    let path = std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| invalid_param(format!("{name} is required for the Microsandbox backend")))?;
    validate_absolute_executable(name, &path)
}

#[cfg(target_os = "linux")]
fn validate_absolute_executable(name: &str, path: &Path) -> Result<PathBuf, ToolError> {
    if !path.is_absolute() {
        return Err(invalid_param(format!("{name} must be an absolute path")));
    }
    let canonical = std::fs::canonicalize(path).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!(
            "{name} points at `{}`, but it cannot be resolved: {err}",
            path.display()
        ),
    })?;
    let meta = std::fs::metadata(&canonical).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("failed to inspect `{}`: {err}", canonical.display()),
    })?;
    if !is_executable(&meta) {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "{name} points at `{}`, but it is not executable",
                canonical.display()
            ),
        });
    }
    reject_untrusted_executable(&canonical, name, &meta)?;
    Ok(canonical)
}

#[cfg(target_os = "linux")]
fn reject_untrusted_executable(
    path: &Path,
    name: &str,
    meta: &std::fs::Metadata,
) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.mode() & 0o022 != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!(
                    "{name} points at `{}`, but it is group/world writable",
                    path.display()
                ),
            });
        }
        let uid = nix::unistd::Uid::current().as_raw();
        if meta.uid() != uid && meta.uid() != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!(
                    "{name} points at `{}`, but it is not owned by the current user or root",
                    path.display()
                ),
            });
        }
    }
    #[cfg(not(unix))]
    let _ = (path, name, meta);
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

fn invalid_param(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".into(),
        message: message.into(),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn image_reference_requires_an_immutable_sha256_digest() {
        for value in [
            "",
            "-q",
            "debian latest",
            "debian:latest",
            "https://registry.example/debian@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "user@registry.example/debian@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "registry.example/debian?tag=latest@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "debian@sha256:abc",
            "debian@sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "debian@sha256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
        ] {
            assert!(validate_image_reference(MSB_IMAGE_ENV, value).is_err());
        }
    }

    #[test]
    fn image_reference_normalizes_the_sha256_digest() {
        let uppercase_digest = "A".repeat(64);
        assert_eq!(
            validate_image_reference(
                MSB_IMAGE_ENV,
                &format!("registry.example:5000/team/debian:stable@sha256:{uppercase_digest}")
            )
            .expect("valid image"),
            format!(
                "registry.example:5000/team/debian:stable@sha256:{}",
                "a".repeat(64)
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_validation_enforces_the_trust_boundary() {
        use std::os::unix::fs::PermissionsExt as _;

        assert!(validate_absolute_executable(MSB_EXE_ENV, Path::new("relative/msb")).is_err());
        assert!(
            validate_absolute_executable(MSB_EXE_ENV, Path::new("/definitely/missing/msb"))
                .is_err()
        );

        let dir = tempfile::tempdir().expect("tempdir");
        assert!(validate_absolute_executable(MSB_EXE_ENV, dir.path()).is_err());
        let executable = dir.path().join("msb");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("write executable");

        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o644))
            .expect("non-executable permissions");
        assert!(validate_absolute_executable(MSB_EXE_ENV, &executable).is_err());

        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o777))
            .expect("writable permissions");
        assert!(validate_absolute_executable(MSB_EXE_ENV, &executable).is_err());

        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("trusted permissions");
        assert_eq!(
            validate_absolute_executable(MSB_EXE_ENV, &executable).expect("trusted executable"),
            std::fs::canonicalize(executable).expect("canonical executable")
        );
    }
}
