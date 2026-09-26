#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
//! Upstream process cleanup pattern + matcher tests.
#![allow(clippy::panic)]

#[cfg(target_os = "linux")]
use crate::gateway::runtime::process_matches_patterns;
use crate::gateway::runtime::upstream_cleanup_patterns;

use super::*;

fn stdio_upstream(name: &str, command: &str, args: Vec<&str>) -> UpstreamConfig {
    UpstreamConfig {
        display_name: None,
        lifecycle: None,
        enabled: true,
        name: name.to_string(),
        url: None,
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: Some(command.to_string()),
        args: args.into_iter().map(str::to_string).collect(),
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

#[test]
fn ssh_cleanup_patterns_use_full_runtime_argv_not_shared_identity_paths() {
    let upstream = stdio_upstream(
        "host-shell",
        "/usr/bin/ssh",
        vec![
            "-i",
            "/tmp/shared-host-key",
            "-o",
            "UserKnownHostsFile=/tmp/shared-known-hosts",
            "tester@192.0.2.10",
            "/opt/tools/shell-mcp",
            "serve",
        ],
    );

    let patterns = upstream_cleanup_patterns(&upstream, false);
    assert_eq!(patterns.len(), 1, "{patterns:?}");
    assert!(patterns[0].contains("/opt/tools/shell-mcp serve"));
    assert!(
        !patterns
            .iter()
            .any(|pattern| pattern == "/tmp/shared-host-key")
    );
    assert!(
        !patterns
            .iter()
            .any(|pattern| pattern == "UserKnownHostsFile=/tmp/shared-known-hosts")
    );

    #[cfg(target_os = "linux")]
    {
        let sibling = "/usr/bin/ssh -i /tmp/shared-host-key -o UserKnownHostsFile=/tmp/shared-known-hosts tester@192.0.2.10 /opt/tools/other-mcp serve";
        assert!(
            !process_matches_patterns(sibling, &patterns),
            "another live upstream on the same SSH identity must not match: {patterns:?}"
        );
    }
}

#[test]
fn ssh_cleanup_patterns_do_not_prefix_match_sibling_upstream_names() {
    let upstream = stdio_upstream(
        "worker",
        "/usr/bin/ssh",
        vec![
            "-i",
            "/tmp/worker",
            "tester@worker-host",
            "/opt/tools/worker.exe",
            "serve",
        ],
    );
    let patterns = upstream_cleanup_patterns(&upstream, false);
    assert_eq!(patterns.len(), 1, "{patterns:?}");

    #[cfg(target_os = "linux")]
    {
        let sibling =
            "/usr/bin/ssh -i /tmp/worker-wsl tester@worker-host /opt/tools/worker-wsl serve";
        assert!(!process_matches_patterns(sibling, &patterns));
    }
}

#[test]
fn github_chat_cleanup_patterns_cover_uv_wrappers() {
    let upstream = UpstreamConfig {
        display_name: None,
        lifecycle: None,
        enabled: true,
        name: "github-chat".to_string(),
        url: None,
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: Some("uvx".to_string()),
        args: vec!["github-chat-mcp".to_string()],
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };

    let patterns = upstream_cleanup_patterns(&upstream, false);
    assert!(patterns.contains(&"github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uvx github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uv tool uvx github-chat-mcp".to_string()));
    assert!(patterns.contains(&"uv run github-chat-mcp".to_string()));
    assert!(patterns.contains(&"github-chat".to_string()));
}

#[cfg(target_os = "linux")]
#[test]
fn process_matcher_uses_joined_cmdline_text() {
    let patterns = vec!["uvx github-chat-mcp".to_string(), "github-chat".to_string()];
    assert!(process_matches_patterns(
        "uvx github-chat-mcp --transport stdio",
        &patterns,
    ));
    assert!(!process_matches_patterns(
        "python -m unrelated-service",
        &patterns,
    ));
}

#[tokio::test]
async fn cleanup_upstream_processes_invalidates_cached_upstream_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(path, runtime.clone());
    let upstream = UpstreamConfig {
        display_name: None,
        lifecycle: None,
        enabled: true,
        name: "cleanup-cached-catalog".to_string(),
        url: None,
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: Some("cleanup-cached-catalog-command".to_string()),
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: true,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };
    manager
        .replace_config_for_tests(vec![upstream.clone()])
        .await;

    let pool = Arc::new(UpstreamPool::new());
    pool.install_test_tools_for_upstream(
        &upstream,
        vec![rmcp::model::Tool::new(
            "open_quick_shell",
            "cached tool metadata",
            Arc::new(serde_json::Map::new()),
        )],
    )
    .await
    .expect("install cached tool");
    runtime.swap(Some(Arc::clone(&pool))).await;
    assert_eq!(pool.healthy_tools().await.len(), 1);

    manager
        .cleanup_upstream_processes(&upstream.name, false, false)
        .await
        .expect("cleanup");

    assert!(pool.healthy_tools().await.is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cleanup_upstream_processes_kills_matching_github_chat_runtime() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    let upstream_name = "github-chat-cleanup-manager";
    let runtime_arg = "github-chat-cleanup-manager-mcp";

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            display_name: None,
            lifecycle: None,
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The cleanup path kills process groups for child runtimes. Keep this
    // stand-in out of nextest's process group so the test process survives.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;

    let _cleanup = manager
        .cleanup_upstream_processes(upstream_name, false, false)
        .await
        .expect("cleanup");

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by cleanup");
}
