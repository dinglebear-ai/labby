//! Restricted spawn selector for the Linux guided CLI MCP adapter.
use std::path::{Path, PathBuf};

use labby_runtime::error::ToolError;

/// Persisted selector, not a program name. Never pass it to PATH lookup.
pub const SELECTOR: &str = "labby::guided-cli";

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "command".into(),
    }
}

/// Validate only the persisted argv shape; host files can disappear between launches.
pub(crate) fn validate_argv(args: &[String]) -> Result<&Path, ToolError> {
    if !cfg!(target_os = "linux") {
        return Err(invalid("guided CLI upstreams are supported on Linux only"));
    }
    let [cli, serve, flag, manifest] = args else {
        return Err(invalid(
            "guided CLI upstream requires its exact manifest invocation",
        ));
    };
    if cli != "cli" || serve != "serve" || flag != "--manifest" {
        return Err(invalid(
            "guided CLI upstream requires its exact manifest invocation",
        ));
    }
    super::spawn_guard::validate_stdio_argv(SELECTOR, args)?;
    let manifest = Path::new(manifest);
    if !manifest.is_absolute() {
        return Err(invalid("guided CLI requires an absolute manifest path"));
    }
    Ok(manifest)
}

/// Resolve the only executable this selector can launch after checking its
/// manifest arguments. The child also parses and validates the manifest.
pub fn prepare(args: &[String]) -> Result<PathBuf, ToolError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        Err(invalid("guided CLI upstreams are supported on Linux only"))
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt as _;

        let manifest = validate_argv(args)?;
        let root = std::env::var_os("LABBY_CLI_MCP_MANIFEST_ROOT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                invalid("guided CLI requires LABBY_CLI_MCP_MANIFEST_ROOT on the host")
            })?;
        validate_manifest(&root, manifest)?;

        let executable = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|_| invalid("the running Labby executable could not be resolved"))?;
        let running = std::fs::metadata("/proc/self/exe")
            .map_err(|_| invalid("the running Labby executable could not be identified"))?;
        let target = executable
            .symlink_metadata()
            .map_err(|_| invalid("the running Labby executable could not be identified"))?;
        if !trusted_ancestors(&executable)
            || !target.is_file()
            || !trusted_metadata(&target)
            || target.mode() & 0o111 == 0
            || (target.dev(), target.ino()) != (running.dev(), running.ino())
        {
            return Err(invalid(
                "the installed Labby executable is not trusted or has changed",
            ));
        }
        Ok(executable)
    }
}

#[cfg(target_os = "linux")]
fn trusted_metadata(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode() & 0o6022 == 0
        && (metadata.uid() == 0 || metadata.uid() == rustix::process::geteuid().as_raw())
}

#[cfg(target_os = "linux")]
fn trusted_ancestors(path: &Path) -> bool {
    path.ancestors().skip(1).all(|directory| {
        directory
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir() && trusted_metadata(&metadata))
    })
}

#[cfg(target_os = "linux")]
fn validate_manifest(root: &Path, manifest: &Path) -> Result<(), ToolError> {
    fn is_exact_canonical(path: &Path) -> bool {
        path.is_absolute()
            && path
                .canonicalize()
                .is_ok_and(|canonical| canonical.as_os_str() == path.as_os_str())
    }

    if root.parent().is_none()
        || !is_exact_canonical(root)
        || !is_exact_canonical(manifest)
        || !manifest.starts_with(root)
        || manifest == root
        || !trusted_ancestors(manifest)
        || !manifest
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && trusted_metadata(&metadata))
    {
        return Err(invalid(
            "guided CLI requires a canonical trusted manifest under the host's approved root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_nonexact_argv_before_resolving_the_manifest() {
        for args in [
            vec!["--version"],
            vec!["cli", "serve", "--manifest"],
            vec![
                "cli",
                "serve",
                "--manifest",
                "/fixture/manifest.toml",
                "--version",
            ],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            assert!(prepare(&args).unwrap_err().to_string().contains("exact"));
        }
    }
}
