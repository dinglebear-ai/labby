//! Restricted spawn selector for the Linux guided CLI MCP adapter.
use std::path::PathBuf;

use labby_runtime::{error::ToolError, gateway_config::UpstreamConfig};

/// Persisted selector, not a program name. Never pass it to PATH lookup.
pub const SELECTOR: &str = "labby::guided-cli";

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "command".into(),
    }
}

fn validate_launch_shape(config: &UpstreamConfig) -> Result<(), ToolError> {
    if !config.env.is_empty()
        || !config.secret_env.is_empty()
        || config.bearer_token_env.is_some()
        || config.lifecycle.is_some()
    {
        return Err(invalid(
            "guided CLI upstreams do not accept environment, credentials, or lifecycle overrides",
        ));
    }
    Ok(())
}

/// Check the exact launch shape again for loaded and reconnecting upstreams.
pub(crate) fn prepare_upstream(
    config: &UpstreamConfig,
    args: &[String],
) -> Result<PathBuf, ToolError> {
    validate_launch_shape(config)?;
    prepare(args)
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
        use std::path::Path;

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
        let root = std::env::var_os("LABBY_CLI_MCP_MANIFEST_ROOT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                invalid("guided CLI requires LABBY_CLI_MCP_MANIFEST_ROOT on the host")
            })?;
        validate_manifest(&root, Path::new(manifest))?;

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
fn trusted_ancestors(path: &std::path::Path) -> bool {
    path.ancestors().skip(1).all(|directory| {
        directory
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir() && trusted_metadata(&metadata))
    })
}

#[cfg(target_os = "linux")]
fn validate_manifest(root: &std::path::Path, manifest: &std::path::Path) -> Result<(), ToolError> {
    fn is_exact_canonical(path: &std::path::Path) -> bool {
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

    #[test]
    fn rejects_runtime_overrides_before_resolving_the_manifest() {
        for overrides in [
            serde_json::json!({"env": {"FIXTURE_OPTION": "fixture"}}),
            serde_json::json!({"secret_env": {"FIXTURE_OPTION": "LABBY_FIXTURE_CREDENTIAL"}}),
            serde_json::json!({"bearer_token_env": "FIXTURE_TOKEN"}),
            serde_json::json!({"lifecycle": "auto"}),
        ] {
            let mut spec = serde_json::json!({"name": "fixture", "command": SELECTOR});
            spec.as_object_mut()
                .unwrap()
                .extend(overrides.as_object().unwrap().clone());
            let config: UpstreamConfig = serde_json::from_value(spec).unwrap();
            assert!(
                prepare_upstream(&config, &config.args)
                    .unwrap_err()
                    .to_string()
                    .contains("overrides")
            );
        }
    }

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
