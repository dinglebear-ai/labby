//! Exercise Skills over a real HTTP transport retained only in a subject shard.
use super::*;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn oauth_http_skills_discovery_and_read_use_only_the_subject_peer() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("JSON-RPC");
            let Some(id) = body.get("id") else {
                return ResponseTemplate::new(202);
            };
            let result = match body["method"].as_str().unwrap_or_default() {
                "initialize" => {
                    let mut capabilities = json!({"tools": {}, "extensions": {}});
                    capabilities["extensions"][labby_runtime::skills::wire::SKILLS_EXTENSION_KEY] =
                        json!({});
                    json!({"protocolVersion": "2025-11-25", "capabilities": capabilities,
                        "serverInfo": {"name": "skills-http", "version": "1"}})
                }
                "tools/list" => json!({"tools": []}),
                "skills/list" => json!({"skills": [entry("up", "alpha")], "ttlMs": 600_000}),
                "resources/read" => json!({"contents": [{"uri": "skill://up/alpha/SKILL.md",
                    "text": skill_md_body("alpha")}]}),
                _ => {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "method not found"}
                    }));
                }
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        })
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("GET"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    let mut config = skills_config("up", None);
    config.command = None;
    config.url = Some(format!("{}/mcp", server.uri()));
    config.lifecycle = Some(labby_runtime::gateway_config::UpstreamLifecycle::Initialize);
    let (connection, tools) =
        super::super::connect::connect_upstream(&config, None, None, None, None)
            .await
            .expect("real HTTP peer");
    config.oauth = oauth_skills_config("up", None).oauth;
    let pool = super::super::UpstreamPool::new();
    pool.ensure_lazy_upstream_entry(&config).await;
    let peer = connection.peer.clone();
    pool.subject_connections.write().await.insert(
        ("up".into(), "alice".into()),
        super::super::SubjectScopedConnection {
            optional_catalogs: Default::default(),
            _connection: connection,
            peer,
            tools: tools.into(),
            last_used: std::time::Instant::now(),
        },
    );
    assert!(pool.connections.read().await.is_empty());
    let catalog = pool
        .upstream_skills(&config, Some("alice"))
        .await
        .expect("HTTP scoped discovery");
    assert_eq!(catalog.skills.len(), 1);
    let file = pool
        .read_proxied_skill_file(&config, Some("alice"), "skill://up/alpha/SKILL.md")
        .await
        .expect("HTTP scoped read");
    assert_eq!(file.bytes, skill_md_body("alpha").into_bytes());
    assert!(
        pool.connections.read().await.is_empty(),
        "subject peer must remain private"
    );
}
