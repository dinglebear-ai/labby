//! Adapter from Add Server's typed OpenAPI form to an ordinary stdio upstream.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

const BIN_ENV: &str = "LABBY_OPENAPI_MCP_BIN";
const ROOT_ENV: &str = "LABBY_OPENAPI_SPEC_ROOT";
const BINARY_NAME: &str = "labby-openapi-mcp";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenApiRegistration {
    pub(crate) spec_path: PathBuf,
    pub(crate) base_url: String,
    pub(crate) allowed_operations: Vec<String>,
    #[serde(default)]
    pub(crate) allow_write: bool,
}

pub(crate) fn command(input: OpenApiRegistration) -> Result<(String, Vec<String>)> {
    let binary =
        env::var_os(BIN_ENV).context("OpenAPI Add Server requires LABBY_OPENAPI_MCP_BIN")?;
    let spec_root =
        env::var_os(ROOT_ENV).context("OpenAPI Add Server requires LABBY_OPENAPI_SPEC_ROOT")?;
    build_command(input, Path::new(&binary), Path::new(&spec_root))
}

fn build_command(
    input: OpenApiRegistration,
    binary: &Path,
    spec_root: &Path,
) -> Result<(String, Vec<String>)> {
    ensure!(
        binary.is_absolute() && spec_root.is_absolute(),
        "the OpenAPI MCP executable and spec root must be absolute gateway-host paths"
    );
    let binary = fs::canonicalize(binary).context("the OpenAPI MCP executable does not exist")?;
    ensure!(
        binary.is_file() && binary.file_name().is_some_and(|name| name == BINARY_NAME),
        "the OpenAPI MCP executable must resolve to a regular labby-openapi-mcp file"
    );
    let spec_root = fs::canonicalize(spec_root).context("the OpenAPI spec root does not exist")?;
    ensure!(
        spec_root.is_dir(),
        "the OpenAPI spec root must be a directory"
    );
    ensure!(
        input.spec_path.is_absolute(),
        "the OpenAPI spec path must be an absolute gateway-host path"
    );
    let spec = fs::canonicalize(input.spec_path).context("the OpenAPI spec file does not exist")?;
    ensure!(
        spec.starts_with(&spec_root)
            && spec.is_file()
            && spec.extension().is_some_and(|ext| ext == "json"),
        "the OpenAPI spec must be a regular .json file inside the approved spec directory"
    );

    let base_url =
        labby_primitives::ssrf::parse_validated_https_url(&input.base_url).map_err(|_| {
            anyhow::anyhow!(
                "the API base URL must be public HTTPS without credentials, query, or fragment"
            )
        })?;
    ensure!(
        (1..=200).contains(&input.allowed_operations.len()),
        "select between 1 and 200 OpenAPI operation IDs"
    );
    let mut seen = BTreeSet::new();
    for operation in &input.allowed_operations {
        ensure!(
            (1..=128).contains(&operation.len())
                && operation
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')),
            "each operation ID must be a valid MCP tool name (letters, digits, underscores, dots or hyphens)"
        );
        ensure!(seen.insert(operation), "operation IDs must be unique");
    }

    let command = binary
        .to_str()
        .context("the executable path must be UTF-8")?;
    let spec = spec.to_str().context("the spec path must be UTF-8")?;
    let mut args = vec![
        format!("--spec-path={spec}"),
        format!("--base-url={base_url}"),
    ];
    args.extend(
        input
            .allowed_operations
            .into_iter()
            .map(|id| format!("--allow-operation={id}")),
    );
    if input.allow_write {
        args.push("--allow-write".into());
    }
    Ok((command.to_owned(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &Path) -> (OpenApiRegistration, PathBuf, PathBuf) {
        let binary = dir.join(BINARY_NAME);
        let spec_root = dir.join("specs");
        fs::write(&binary, b"not executed in these tests").unwrap();
        fs::create_dir(&spec_root).unwrap();
        let spec_path = spec_root.join("api.json");
        fs::write(&spec_path, b"{}").unwrap();
        (
            OpenApiRegistration {
                spec_path,
                base_url: "https://api.example.com/v1".into(),
                allowed_operations: vec!["listItems".into(), "--not-an-option".into()],
                allow_write: false,
            },
            binary,
            spec_root,
        )
    }

    #[test]
    fn builds_only_fixed_flags_without_values_becoming_options() {
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allow_write = true;
        let expected_path = fs::canonicalize(&input.spec_path).unwrap();
        let (command, args) = build_command(input, &binary, &root).unwrap();
        assert_eq!(Path::new(&command), fs::canonicalize(binary).unwrap());
        assert_eq!(
            args,
            [
                format!("--spec-path={}", expected_path.display()),
                "--base-url=https://api.example.com/v1".into(),
                "--allow-operation=listItems".into(),
                "--allow-operation=--not-an-option".into(),
                "--allow-write".into(),
            ]
        );
    }

    #[test]
    fn rejects_invalid_or_duplicate_operations_and_untrusted_urls() {
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allowed_operations = vec!["listItems".into(), "listItems".into()];
        assert!(build_command(input, &binary, &root).is_err());
        for url in [
            "http://api.example.com",
            "https://127.0.0.1",
            "https://api.example.com/?key=value",
        ] {
            let input = OpenApiRegistration {
                spec_path: root.join("api.json"),
                base_url: url.into(),
                allowed_operations: vec!["listItems".into()],
                allow_write: false,
            };
            assert!(build_command(input, &binary, &root).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_spec_symlink_outside_approved_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (input, binary, root) = fixture(dir.path());
        let outside = dir.path().join("other.json");
        fs::write(&outside, b"{}").unwrap();
        fs::remove_file(&input.spec_path).unwrap();
        std::os::unix::fs::symlink(outside, &input.spec_path).unwrap();
        assert!(build_command(input, &binary, &root).is_err());
    }

    #[test]
    fn rejects_raw_secret_or_command_fields_and_defaults_to_read_only() {
        let params = serde_json::json!({
            "spec_path": "/opt/labby/specs/api.json",
            "base_url": "https://api.example.com",
            "allowed_operations": ["listItems"]
        });
        let parsed: OpenApiRegistration = serde_json::from_value(params.clone()).unwrap();
        assert!(!parsed.allow_write);
        for key in ["command", "secret_env_values", "bearer_token", "spec_url"] {
            let mut params = params.clone();
            params[key] = serde_json::json!("not supported");
            assert!(serde_json::from_value::<OpenApiRegistration>(params).is_err());
        }
    }
}
