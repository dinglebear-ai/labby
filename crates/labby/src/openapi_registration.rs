//! Adapter from Add Server's typed OpenAPI form to an ordinary stdio upstream.

use std::path::PathBuf;

use anyhow::{Context, Result};
use labby_gateway::security::guided_openapi::{HostPaths, SELECTOR};
use labby_openapi::registry::preflight_mcp_path;
use serde::Deserialize;

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
    build_command(input, &HostPaths::from_env()?)
}

fn build_command(input: OpenApiRegistration, paths: &HostPaths) -> Result<(String, Vec<String>)> {
    let spec = paths.canonical_spec(&input.spec_path)?;

    let base_url =
        labby_primitives::ssrf::parse_validated_https_url(&input.base_url).map_err(|_| {
            anyhow::anyhow!(
                "the API base URL must be public HTTPS without credentials, query, or fragment"
            )
        })?;
    preflight_mcp_path(&spec, &input.allowed_operations, input.allow_write)?;

    let spec = spec.to_str().context("the spec path must be UTF-8")?;
    let mut args = vec![
        // The stdio sandbox discovers host paths as individual argv tokens.
        "--spec-path".into(),
        spec.to_owned(),
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
    Ok((SELECTOR.into(), args))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;

    fn fixture_command(
        input: OpenApiRegistration,
        binary: &Path,
        root: &Path,
    ) -> Result<(String, Vec<String>)> {
        build_command(input, &HostPaths::new(binary, root)?)
    }

    const FIXTURE_SPEC: &str = r#"{
        "openapi": "3.0.0",
        "paths": {
            "/items": {
                "get": {"operationId": "listItems"},
                "post": {"operationId": "--not-an-option"}
            },
            "/catalog": {"head": {"operationId": "catalog.get-one"}},
            "/ambiguous-a": {"get": {"operationId": "duplicateItem"}},
            "/ambiguous-b": {"get": {"operationId": "duplicateItem"}},
            "/unsupported": {"trace": {"operationId": "traceItem"}}
        }
    }"#;

    fn fixture(dir: &Path) -> (OpenApiRegistration, PathBuf, PathBuf) {
        let binary = dir.join(format!("labby-openapi-mcp{}", std::env::consts::EXE_SUFFIX));
        let spec_root = dir.join("specs");
        fs::write(&binary, b"not executed in these tests").unwrap();
        fs::create_dir(&spec_root).unwrap();
        let spec_path = spec_root.join("api.json");
        fs::write(&spec_path, FIXTURE_SPEC).unwrap();
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
        let (command, args) = fixture_command(input, &binary, &root).unwrap();
        assert_eq!(command, SELECTOR);
        assert_eq!(
            args,
            [
                "--spec-path".into(),
                expected_path.to_str().unwrap().to_owned(),
                "--base-url=https://api.example.com/v1".into(),
                "--allow-operation=listItems".into(),
                "--allow-operation=--not-an-option".into(),
                "--allow-write".into(),
            ]
        );
    }

    #[test]
    fn canonicalizes_host_inputs_and_keeps_default_registration_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        let spaced_spec = root.join("approved spec.json");
        fs::rename(&input.spec_path, &spaced_spec).unwrap();
        input.spec_path = root.join(".").join("approved spec.json");
        input.base_url = "https://API.EXAMPLE.COM:443/v1".into();
        input.allowed_operations = vec!["listItems".into(), "catalog.get-one".into()];
        let canonical = fs::canonicalize(spaced_spec).unwrap();
        let path_arg = canonical.to_str().unwrap().to_owned();
        let (command, args) = fixture_command(input, &binary, &root).unwrap();
        assert_eq!(command, SELECTOR);
        assert_eq!(
            args,
            [
                "--spec-path".into(),
                path_arg,
                "--base-url=https://api.example.com/v1".into(),
                "--allow-operation=listItems".into(),
                "--allow-operation=catalog.get-one".into(),
            ]
        );
    }

    #[test]
    fn enforces_the_operation_count_and_tool_name_boundaries() {
        let operations = |count| (0..count).map(|i| format!("operation-{i}")).collect();
        for (ids, expected_message) in [
            (
                operations(0),
                "select between 1 and 200 OpenAPI operation IDs",
            ),
            (
                operations(201),
                "select between 1 and 200 OpenAPI operation IDs",
            ),
            (
                vec!["".into()],
                "each operation ID must be a valid MCP tool name",
            ),
            (
                vec!["a".repeat(129)],
                "each operation ID must be a valid MCP tool name",
            ),
            (
                vec!["with space".into()],
                "each operation ID must be a valid MCP tool name",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (mut input, binary, root) = fixture(dir.path());
            input.allowed_operations = ids;
            assert!(
                fixture_command(input, &binary, &root)
                    .unwrap_err()
                    .to_string()
                    .starts_with(expected_message)
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allowed_operations = operations(199);
        input.allowed_operations.push("x".repeat(128));
        let paths = input
            .allowed_operations
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    format!("/path-{i}"),
                    serde_json::json!({"get": {"operationId": id}}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        fs::write(
            &input.spec_path,
            serde_json::to_vec(&serde_json::json!({"openapi": "3.0.0", "paths": paths})).unwrap(),
        )
        .unwrap();
        let (_, args) = fixture_command(input, &binary, &root).unwrap();
        assert_eq!(args.len(), 203);
        assert_eq!(args[202], format!("--allow-operation={}", "x".repeat(128)));
    }

    #[test]
    fn rejects_invalid_or_duplicate_operations_and_untrusted_urls() {
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allowed_operations = vec!["listItems".into(), "listItems".into()];
        assert!(fixture_command(input, &binary, &root).is_err());
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
            assert!(fixture_command(input, &binary, &root).is_err());
        }
    }

    #[test]
    fn rejects_missing_ambiguous_unsupported_or_unapproved_write_operations() {
        for (operation, message) in [
            ("missing", "is absent"),
            ("duplicateItem", "has duplicate IDs"),
            ("traceItem", "unsupported input metadata"),
            ("--not-an-option", "is not read-only"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (mut input, binary, root) = fixture(dir.path());
            input.allowed_operations = vec![operation.into()];
            let error = fixture_command(input, &binary, &root).unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn rejects_malformed_or_oversized_specs_before_building_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allowed_operations = vec!["listItems".into()];
        fs::write(&input.spec_path, b"not JSON; private document content").unwrap();
        let error = fixture_command(input, &binary, &root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to parse"), "{error}");
        assert!(!error.contains("private document content"), "{error}");

        let dir = tempfile::tempdir().unwrap();
        let (mut input, binary, root) = fixture(dir.path());
        input.allowed_operations = vec!["listItems".into()];
        fs::OpenOptions::new()
            .write(true)
            .open(&input.spec_path)
            .unwrap()
            .set_len(4 * 1024 * 1024 + 1)
            .unwrap();
        let error = fixture_command(input, &binary, &root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds the size cap"), "{error}");
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
        assert!(fixture_command(input, &binary, &root).is_err());
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
