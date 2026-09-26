//! A scope filter must prevent upstream discovery, not merely hide its output.

use super::*;
use crate::skills::aggregate::ToolAccess;
use crate::skills::facade::{SkillCallerScope, register_code_mode_skill_context};
use labby_codemode::CodeModeCallerCapabilities;
use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
use labby_gateway::upstream::pool::UpstreamPool;
use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};
use std::time::Duration;

#[tokio::test]
async fn discovery_scope_does_not_contact_an_excluded_skill_upstream() {
    // No response is served. An erroneous discovery call must stall rather
    // than accidentally pass because a mock immediately returned an error.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = UpstreamConfig {
        display_name: None,
        lifecycle: None,
        enabled: true,
        name: "excluded".to_string(),
        url: Some(format!("http://{address}/mcp")),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: Default::default(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: true,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };
    let runtime = GatewayRuntimeHandle::default();
    runtime.swap(Some(Arc::new(UpstreamPool::new()))).await;
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(GatewayManager::new(
        root.path().join("config.toml"),
        runtime,
    ));
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![upstream],
            ..GatewayConfig::default()
        })
        .await;
    let guard = register_code_mode_skill_context(SkillRegistryContext::with_manager(
        manager,
        SkillCallerScope::root(Some("alice".to_string()), ToolAccess::CodeModeOnly),
    ));
    let caller = CodeModeCaller::ScopedSkills {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            ..Default::default()
        },
        sub: Some("alice".to_string()),
        skill_context_token: guard.token().to_string(),
    };
    let scope = ToolScope::new(vec!["macpoo".to_string()], Vec::new());
    let skills = tokio::time::timeout(
        Duration::from_secs(1),
        CanonicalCodeModeSkillProvider.list(&caller, &scope),
    )
    .await
    .expect("out-of-scope discovery must not delay the Code Mode call")
    .unwrap();
    assert!(skills.iter().any(|skill| skill.name == "using-labby"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "the excluded upstream must receive zero TCP connections"
    );
}
