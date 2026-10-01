use super::*;

fn config_at(temp: &tempfile::TempDir, servers: serde_json::Value) -> PathBuf {
    let path = temp.path().join(".mcp.json");
    std::fs::write(
        &path,
        serde_json::json!({"mcpServers": servers}).to_string(),
    )
    .unwrap();
    path
}

fn isolated_root(prepared: &PreparedMcpJson) -> PathBuf {
    prepared
        .child_env
        .iter()
        .find(|(name, _)| name == "LABBY_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .unwrap()
}

#[test]
fn mixed_config_reuses_gateway_transports_and_preserves_private_bearer() {
    let temp = tempfile::tempdir().unwrap();
    let token = "literal-$NOT_AN_ENV_VAR-\\-\"-secret";
    let path = config_at(
        &temp,
        serde_json::json!({
            "local": {"command":"node", "args":["server.js"]},
            "remote": {"type":"streamable-http", "url":"https://example.com/mcp", "headers":{"Authorization":format!("Bearer {token}"), "X-Workspace":"demo"}}
        }),
    );
    let prepared = prepare(&path).unwrap();
    let root = isolated_root(&prepared);
    let config: IsolatedConfigRead =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config.upstream[0].effective_transport(),
        Some(UpstreamTransport::Stdio)
    );
    let remote = &config.upstream[1];
    assert_eq!(remote.effective_transport(), Some(UpstreamTransport::Http));
    assert_eq!(remote.headers["X-Workspace"], "demo");
    assert!(
        remote
            .headers
            .keys()
            .all(|name| !name.eq_ignore_ascii_case("authorization"))
    );
    assert!(
        !std::fs::read_to_string(root.join("config.toml"))
            .unwrap()
            .contains(token)
    );
    let credentials = dotenvy::from_path_iter(root.join(".env"))
        .unwrap()
        .collect::<Result<BTreeMap<_, _>, _>>()
        .unwrap();
    assert_eq!(
        credentials[remote.bearer_token_env.as_ref().unwrap()],
        token
    );
    assert!(config.upstream[0].env.is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(root.join(".env"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[derive(Deserialize)]
struct IsolatedConfigRead {
    upstream: Vec<UpstreamConfig>,
}

#[test]
fn bearer_header_cannot_be_overridden_by_predictable_ambient_names() {
    let temp = tempfile::tempdir().unwrap();
    let path = config_at(
        &temp,
        serde_json::json!({"remote": {
            "url":"https://example.com/mcp", "headers":{"Authorization":"Bearer selected-secret"}
        }}),
    );
    let prepared = prepare_with_overrides(
        &path,
        None,
        &[("LABBY_AGGREGATE_TOKEN_0".into(), "wrong-secret".into())],
        &[],
    )
    .unwrap();
    let root = isolated_root(&prepared);
    let config: IsolatedConfigRead =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    let key = config.upstream[0].bearer_token_env.as_ref().unwrap();
    assert_ne!(key, "LABBY_AGGREGATE_TOKEN_0");
    let values = dotenvy::from_path_iter(root.join(".env"))
        .unwrap()
        .collect::<Result<BTreeMap<_, _>, _>>()
        .unwrap();
    assert_eq!(values[key], "selected-secret");
}

#[test]
fn stdio_bearer_reference_keeps_the_childs_expected_environment_name() {
    let temp = tempfile::tempdir().unwrap();
    let path = config_at(
        &temp,
        serde_json::json!({"local": {
            "command":"node", "bearer_token_env":"API_TOKEN", "env":{"API_TOKEN":"selected-secret"}
        }}),
    );
    let prepared = prepare(&path).unwrap();
    let root = isolated_root(&prepared);
    let config: IsolatedConfigRead =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    assert_eq!(config.upstream[0].env["API_TOKEN"], "selected-secret");
    assert!(config.upstream[0].bearer_token_env.is_none());
}

#[test]
fn gateway_rejects_invalid_or_conflicting_http_configuration() {
    for entry in [
        serde_json::json!({"url":"file:///etc/passwd"}),
        serde_json::json!({"url":"https://example.com/mcp", "command":"node"}),
        serde_json::json!({"type":"stdio", "url":"https://example.com/mcp"}),
        serde_json::json!({"type":"sse", "url":"https://example.com/sse"}),
        serde_json::json!({"type":"http", "transport":"stdio", "url":"https://example.com/mcp"}),
        serde_json::json!({"url":"https://example.com/mcp", "headers":{"X-Key":"bad\r\nInjected: yes"}}),
        serde_json::json!({"url":"https://example.com/mcp", "headers":{"Authorization":"Basic secret"}}),
        serde_json::json!({"command":"node", "headers":{"Authorization":"Bearer secret"}}),
        serde_json::json!({"url":"https://example.com/mcp", "headers":{"Authorization":"Bearer secret"}, "bearer_token_env":"TOKEN"}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = config_at(&temp, serde_json::json!({"invalid": entry}));
        assert!(prepare(&path).is_err());
    }
}

#[test]
fn bearer_reference_uses_entry_value_without_forwarding_it_to_stdio() {
    let temp = tempfile::tempdir().unwrap();
    let path = config_at(
        &temp,
        serde_json::json!({
            "remote": {"url":"https://example.com/mcp", "bearer_token_env":"HTTP_TOKEN", "env":{"HTTP_TOKEN":"entry-token"}},
            "local": {"command":"node"}
        }),
    );
    let prepared = prepare(&path).unwrap();
    let root = isolated_root(&prepared);
    let values = dotenvy::from_path_iter(root.join(".env"))
        .unwrap()
        .collect::<Result<BTreeMap<_, _>, _>>()
        .unwrap();
    assert_eq!(values.values().next().unwrap(), "entry-token");
    let config: IsolatedConfigRead =
        toml::from_str(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    assert!(
        config
            .upstream
            .iter()
            .all(|upstream| upstream.env.is_empty())
    );
}

#[test]
fn discovery_prefers_home_then_executable_directory() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("labby");
    assert!(discover(&home, &executable).unwrap().is_none());
    let beside_binary = bin.join(".mcp.json");
    std::fs::write(&beside_binary, "{}").unwrap();
    assert_eq!(discover(&home, &executable).unwrap(), Some(beside_binary));
    let in_home = home.join(".mcp.json");
    std::fs::write(&in_home, "invalid JSON").unwrap();
    let selected = discover(&home, &executable).unwrap().unwrap();
    assert_eq!(selected, in_home);
    assert!(
        prepare(&selected).is_err(),
        "invalid home config must not fall back"
    );
}

#[test]
fn discovery_rejects_a_directory_instead_of_falling_back() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".mcp.json")).unwrap();
    std::fs::write(temp.path().join(".mcp.json"), "{}").unwrap();
    let error = discover(&home, &temp.path().join("labby")).unwrap_err();
    assert!(error.to_string().contains("not a file"));
}
