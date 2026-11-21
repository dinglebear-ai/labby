//! Managed OpenAPI stdio launch: host-provisioned executable and a local spec only.
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use labby_openapi::registry::validate_mcp_operation_ids;
use labby_runtime::error::ToolError;

/// Persisted selector, never a PATH command.
pub const SELECTOR: &str = "labby::guided-openapi";
const BIN_ENV: &str = "LABBY_OPENAPI_MCP_BIN";
const ROOT_ENV: &str = "LABBY_OPENAPI_SPEC_ROOT";
#[cfg(windows)]
const BINARY_NAME: &str = "labby-openapi-mcp.exe";
#[cfg(not(windows))]
const BINARY_NAME: &str = "labby-openapi-mcp";

/// Canonical host paths for the guided adapter. These are not read from an upstream.
pub struct HostPaths {
    binary: PathBuf,
    spec_root: PathBuf,
}

impl HostPaths {
    /// Resolve the host operator's configured binary and spec root.
    pub fn from_env() -> Result<Self> {
        let binary = std::env::var_os(BIN_ENV)
            .context("OpenAPI Add Server requires LABBY_OPENAPI_MCP_BIN")?;
        let root = std::env::var_os(ROOT_ENV)
            .context("OpenAPI Add Server requires LABBY_OPENAPI_SPEC_ROOT")?;
        Self::new(Path::new(&binary), Path::new(&root))
    }

    /// Check the host paths before preparing a typed registration.
    pub fn new(binary: &Path, spec_root: &Path) -> Result<Self> {
        ensure!(
            binary.is_absolute() && spec_root.is_absolute(),
            "the OpenAPI MCP executable and spec root must be absolute gateway-host paths"
        );
        let binary = binary
            .canonicalize()
            .context("the OpenAPI MCP executable does not exist")?;
        ensure!(
            binary.is_file() && binary.file_name().is_some_and(|name| name == BINARY_NAME),
            "the OpenAPI MCP executable must resolve to a regular {BINARY_NAME} file"
        );
        let spec_root = spec_root
            .canonicalize()
            .context("the OpenAPI spec root does not exist")?;
        ensure!(
            spec_root.is_dir(),
            "the OpenAPI spec root must be a directory"
        );
        Ok(Self { binary, spec_root })
    }

    /// Resolve a local JSON spec underneath the approved directory.
    pub fn canonical_spec(&self, path: &Path) -> Result<PathBuf> {
        ensure!(
            path.is_absolute(),
            "the OpenAPI spec path must be an absolute gateway-host path"
        );
        let spec = path
            .canonicalize()
            .context("the OpenAPI spec file does not exist")?;
        ensure!(
            spec.starts_with(&self.spec_root)
                && spec.is_file()
                && spec.extension().is_some_and(|ext| ext == "json"),
            "the OpenAPI spec must be a regular .json file inside the approved spec directory"
        );
        Ok(spec)
    }
}

pub(crate) fn prepare(args: &[String]) -> Result<PathBuf, ToolError> {
    validate_argv(args)?;
    launch_under(args, HostPaths::from_env().map_err(invalid)?)
}

fn validate_argv(args: &[String]) -> Result<(), ToolError> {
    super::spawn_guard::validate_stdio_argv(SELECTOR, args)?;
    let [flag, _, base, operations @ ..] = args else {
        return Err(invalid(
            "guided OpenAPI upstream requires its exact local spec invocation",
        ));
    };
    if flag != "--spec-path" {
        return Err(invalid(
            "guided OpenAPI upstream requires its exact local spec invocation",
        ));
    }
    let base = base
        .strip_prefix("--base-url=")
        .ok_or_else(|| invalid("guided OpenAPI upstream requires an API base URL"))?;
    labby_primitives::ssrf::parse_validated_https_url(base).map_err(|_| {
        invalid("the API base URL must be public HTTPS without credentials, query, or fragment")
    })?;
    let operations = if operations.last().is_some_and(|arg| arg == "--allow-write") {
        &operations[..operations.len() - 1]
    } else {
        operations
    };
    let ids = operations
        .iter()
        .map(|arg| {
            arg.strip_prefix("--allow-operation=")
                .map(str::to_owned)
                .ok_or_else(|| {
                    invalid("guided OpenAPI upstream permits only explicit operation IDs")
                })
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    validate_mcp_operation_ids(&ids).map_err(invalid)
}

fn launch_under(args: &[String], paths: HostPaths) -> Result<PathBuf, ToolError> {
    // Callers first validate the exact argv grammar; the path is its own token
    // so the normal stdio sandbox can grant that file (not its directory).
    let path = Path::new(&args[1]);
    let canonical = paths.canonical_spec(path).map_err(invalid)?;
    if path.as_os_str() != canonical.as_os_str() {
        return Err(invalid(
            "guided OpenAPI upstream requires a canonical spec path",
        ));
    }
    Ok(paths.binary)
}

fn invalid(error: impl std::fmt::Display) -> ToolError {
    ToolError::InvalidParam {
        message: error.to_string(),
        param: "command".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(path: &Path) -> Vec<String> {
        vec![
            "--spec-path".into(),
            path.to_str().unwrap().into(),
            "--base-url=https://catalog.example.invalid/v1".into(),
            "--allow-operation=getItem".into(),
        ]
    }

    #[test]
    fn managed_launch_uses_the_canonical_configured_platform_executable_and_local_spec() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join(BINARY_NAME);
        std::fs::write(&binary, "fixture; not executed").unwrap();
        let root = directory.path().join("specs");
        std::fs::create_dir(&root).unwrap();
        let spec = root.join("local fixture.json");
        std::fs::write(&spec, "{}").unwrap();
        let canonical = spec.canonicalize().unwrap();
        let approved = || HostPaths::new(&binary, &root).unwrap();

        let mut argv = args(&canonical);
        validate_argv(&argv).unwrap();
        argv.push("--allow-write".into());
        validate_argv(&argv).unwrap();
        assert_eq!(
            launch_under(&argv, approved()).unwrap(),
            binary.canonicalize().unwrap()
        );
        assert_eq!(
            BINARY_NAME,
            format!("labby-openapi-mcp{}", std::env::consts::EXE_SUFFIX)
        );

        let outside = directory.path().join("other.json");
        std::fs::write(&outside, "{}").unwrap();
        assert!(launch_under(&args(&outside.canonicalize().unwrap()), approved()).is_err());
        let noncanonical = canonical
            .parent()
            .unwrap()
            .join(".")
            .join(canonical.file_name().unwrap());
        assert!(launch_under(&args(&noncanonical), approved()).is_err());
    }

    #[test]
    fn rejects_non_managed_argv_before_resolving_host_paths() {
        let mut invocation = args(Path::new("/fixture/spec.json"));
        invocation.push("--diagnostic-option".into());
        assert!(validate_argv(&invocation).is_err());
        invocation.pop();
        invocation.push("--allow-operation=getItem".into());
        assert!(validate_argv(&invocation).is_err());
        invocation.truncate(3);
        assert!(validate_argv(&invocation).is_err());
        invocation.push("--allow-operation=getItem".into());
        invocation[2] = "--base-url=http://catalog.example.invalid".into();
        assert!(validate_argv(&invocation).is_err());
        invocation[0] = "--spec-url".into();
        assert!(validate_argv(&invocation).is_err());
    }
}
