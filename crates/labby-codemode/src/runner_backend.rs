//! Resolve the process boundary used for the Javy/QuickJS runner.
//!
//! The default remains a direct self re-exec. Operators may opt into a
//! Microsandbox microVM; only the runner crosses that boundary. MCP discovery,
//! authorization, dispatch, and credentials remain in the Labby host process.

use std::path::{Path, PathBuf};

use crate::error::ToolError;
use crate::pool::{MicrosandboxSpawn, RunnerSpawn};

const BACKEND_ENV: &str = "LABBY_CODE_MODE_RUNNER_BACKEND";
const MSB_EXE_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_EXE";
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
        let runner = super::runner_exe::resolve_runner_exe()?;
        let msb = required_absolute_executable(MSB_EXE_ENV)?;
        let image = required_nonempty(MSB_IMAGE_ENV)?;
        tracing::info!(
            backend = "microsandbox",
            image,
            runner = %runner.display(),
            msb = %msb.display(),
            network = "disabled",
            "using Microsandbox for Code Mode runner isolation"
        );

        Ok((
            RunnerSpawn {
                program: runner,
                args: runner_args(),
            },
            Some(MicrosandboxSpawn {
                executable: msb,
                image,
            }),
        ))
    }
}

fn runner_args() -> Vec<String> {
    vec!["internal".into(), "code-mode-runner".into()]
}

fn required_nonempty(name: &str) -> Result<String, ToolError> {
    let value = std::env::var(name)
        .map_err(|_| invalid_param(format!("{name} is required for the Microsandbox backend")))?;
    validate_image_reference(name, &value)
}

fn validate_image_reference(name: &str, value: &str) -> Result<String, ToolError> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err(invalid_param(format!(
            "{name} must be one non-empty OCI image reference"
        )));
    }
    Ok(value.to_string())
}

fn required_absolute_executable(name: &str) -> Result<PathBuf, ToolError> {
    let path = std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| invalid_param(format!("{name} is required for the Microsandbox backend")))?;
    validate_absolute_executable(name, &path)
}

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
    if !is_executable(&canonical) {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "{name} points at `{}`, but it is not executable",
                canonical.display()
            ),
        });
    }
    reject_untrusted_executable(&canonical, name)?;
    Ok(canonical)
}

fn reject_untrusted_executable(path: &Path, name: &str) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::metadata(path).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to inspect `{}`: {err}", path.display()),
        })?;
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
    let _ = (path, name);
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reference_rejects_option_injection_and_whitespace() {
        for value in ["", "-q", "debian latest"] {
            assert!(validate_image_reference(MSB_IMAGE_ENV, value).is_err());
        }
        assert_eq!(
            validate_image_reference(MSB_IMAGE_ENV, "debian@sha256:abc").expect("valid image"),
            "debian@sha256:abc"
        );
    }

    #[test]
    fn direct_runner_args_remain_protocol_compatible() {
        assert_eq!(runner_args(), ["internal", "code-mode-runner"]);
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
