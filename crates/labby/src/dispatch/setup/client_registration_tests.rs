use super::*;
fn home() -> tempfile::TempDir {
    #[cfg(target_os = "macos")]
    let parent = PathBuf::from("/private/tmp");
    #[cfg(not(target_os = "macos"))]
    let parent = std::env::temp_dir().canonicalize().unwrap();
    tempfile::Builder::new()
        .prefix("lab-clients-")
        .tempdir_in(parent)
        .unwrap()
}
#[test]
fn preserve_backup_and_idempotently_register_both_clients() {
    for (client, original) in [
        (
            ExternalClient::Codex,
            "# Keep comment\nmodel = 'example'\n[mcp_servers.other]\ncommand = 'other'\n",
        ),
        (
            ExternalClient::ClaudeCode,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
        ),
    ] {
        let directory = home();
        let path = directory.path().join(client.relative_path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, original).unwrap();
        let proposed = plan(directory.path(), client, "https://example.com/mcp").unwrap();
        let result = register(
            directory.path(),
            client,
            &proposed.gateway_url,
            &proposed.config_version,
        )
        .unwrap();
        assert!(result.changed);
        assert!(!result.connected);
        assert!(!result.plan.readiness_supported);
        assert!(
            result
                .plan
                .readiness_limitation
                .unwrap()
                .contains("cannot complete")
        );
        assert_eq!(
            std::fs::read_to_string(result.backup_path.unwrap()).unwrap(),
            original
        );
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("other"));
        assert!(!updated.contains("bearer"));
        assert!(!updated.contains("Authorization"));
        let proposed = plan(directory.path(), client, "https://example.com/mcp").unwrap();
        assert!(
            !register(
                directory.path(),
                client,
                &proposed.gateway_url,
                &proposed.config_version
            )
            .unwrap()
            .changed
        );
    }
}
#[test]
fn malformed_configuration_errors_do_not_echo_existing_secrets() {
    let error = render(
        ExternalClient::Codex,
        "secret = 'DO_NOT_PRINT\n",
        "https://example.com/mcp",
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains("DO_NOT_PRINT"));
    let error = render(
        ExternalClient::ClaudeCode,
        "{\"token\":\"DO_NOT_PRINT",
        "https://example.com/mcp",
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains("DO_NOT_PRINT"));
}

#[cfg(unix)]
#[test]
fn refuse_symlinked_client_configuration() {
    let directory = home();
    let outside = home();
    std::os::unix::fs::symlink(outside.path(), directory.path().join(".codex")).unwrap();
    assert!(
        plan(
            directory.path(),
            ExternalClient::Codex,
            "https://example.com/mcp"
        )
        .is_err()
    );
    assert!(!outside.path().join("config.toml").exists());
}

#[test]
fn codex_inline_tables_preserve_other_servers() {
    let output = render(
        ExternalClient::Codex,
        "mcp_servers = { other = { command = 'other' } }\n",
        "https://example.com/mcp",
    )
    .unwrap();
    let parsed: toml::Value = toml::from_str(&output).unwrap();
    assert_eq!(
        parsed["mcp_servers"]["other"]["command"].as_str(),
        Some("other")
    );
    assert_eq!(
        parsed["mcp_servers"]["lab"]["url"].as_str(),
        Some("https://example.com/mcp")
    );
}

#[test]
fn conflict_and_changed_snapshot_do_not_overwrite() {
    let directory = home();
    let path = directory.path().join(".claude.json");
    let proposed = plan(
        directory.path(),
        ExternalClient::ClaudeCode,
        "https://example.com/mcp",
    )
    .unwrap();
    std::fs::write(&path, "{\"other\":true}").unwrap();
    assert!(
        register(
            directory.path(),
            ExternalClient::ClaudeCode,
            &proposed.gateway_url,
            &proposed.config_version
        )
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"other\":true}");
    std::fs::write(
        &path,
        "{\"mcpServers\":{\"lab\":{\"command\":\"existing\"}}}",
    )
    .unwrap();
    assert!(
        plan(
            directory.path(),
            ExternalClient::ClaudeCode,
            "https://example.com/mcp"
        )
        .is_err()
    );
}
#[tokio::test]
async fn registration_qualifies_oauth_and_rejects_bearer_only_gateway_before_writing() {
    drop(rustls::crypto::ring::default_provider().install_default());
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    let gateway = format!("{}/mcp", server.uri());
    let directory = home();
    assert!(
        verified_plan(
            directory.path().to_path_buf(),
            ExternalClient::ClaudeCode,
            gateway.clone()
        )
        .await
        .is_err()
    );
    assert!(
        verified_register(
            directory.path().to_path_buf(),
            ExternalClient::ClaudeCode,
            gateway.clone(),
            version("")
        )
        .await
        .is_err()
    );
    assert!(!directory.path().join(".claude.json").exists());
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"resource":gateway,"authorization_servers":[server.uri()]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"issuer":server.uri(),"authorization_endpoint":format!("{}/authorize",server.uri()),"token_endpoint":format!("{}/token",server.uri()),"registration_endpoint":format!("{}/register",server.uri()),"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"]}))).mount(&server).await;
    let plan = verified_plan(
        directory.path().to_path_buf(),
        ExternalClient::ClaudeCode,
        gateway.clone(),
    )
    .await
    .unwrap();
    let result = verified_register(
        directory.path().to_path_buf(),
        ExternalClient::ClaudeCode,
        gateway,
        plan.config_version,
    )
    .await
    .unwrap();
    assert!(result.changed);
    assert!(!result.connected);
}

#[test]
fn oauth_resource_must_bind_selected_gateway() {
    let gateway = validated_endpoint("https://example.com/mcp").unwrap();
    assert!(validate_resource_metadata(&gateway, &serde_json::json!({"resource":"https://example.com/other","authorization_servers":["https://example.com"]})).is_err());
    assert!(validate_resource_metadata(&gateway, &serde_json::json!({"resource":"https://example.com/mcp","authorization_servers":["https://example.com"]})).is_ok());
    for endpoint in [
        "https://exa\tmple.com/mcp",
        "https://example.com/\\mcp",
        " https://example.com/mcp",
        "http://example.com/mcp",
        "https://token@example.com/mcp",
        "https://example.com/mcp?token=x",
    ] {
        assert!(validated_endpoint(endpoint).is_err());
    }
}

#[test]
fn detects_fixture_configs_and_preserves_owner_only_backups() {
    let directory = home();
    let path = directory.path().join(".claude.json");
    std::fs::write(&path, "{}\n").unwrap();
    let detected = detect(directory.path());
    assert!(
        detected
            .iter()
            .any(|entry| matches!(entry.client, ExternalClient::ClaudeCode)
                && entry.configuration_exists)
    );
    let proposed = plan(
        directory.path(),
        ExternalClient::ClaudeCode,
        "https://example.com/mcp",
    )
    .unwrap();
    let result = register(
        directory.path(),
        ExternalClient::ClaudeCode,
        &proposed.gateway_url,
        &proposed.config_version,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(result.backup_path.unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(not(unix))]
    assert!(result.backup_path.is_some());
}

#[test]
fn saved_bridge_registration_preserves_clients_without_copying_credentials() {
    for client in [ExternalClient::Codex, ExternalClient::ClaudeCode] {
        let directory = home();
        let bridge = super::super::client_bridge::BridgeDescriptor {
            command: "/owned/labby".into(),
            args: vec![
                "setup".into(),
                "clients".into(),
                "bridge".into(),
                "--state-root".into(),
                "/owned/state".into(),
            ],
        };
        let proposed = plan_connection(
            directory.path(),
            client,
            "http://127.0.0.1:8765",
            Some(bridge.clone()),
        )
        .unwrap();
        let result = register_connection(
            directory.path(),
            client,
            &proposed.gateway_url,
            &proposed.config_version,
            Some(bridge),
        )
        .unwrap();
        assert!(result.bridge_verified);
        assert!(!result.connected);
        let raw = std::fs::read_to_string(result.plan.config_path).unwrap();
        assert!(raw.contains("/owned/labby"));
        assert!(raw.contains("--state-root"));
        assert!(!raw.contains("LABBY_MCP_HTTP_TOKEN"));
        assert!(!raw.contains("Authorization"));
    }
}
