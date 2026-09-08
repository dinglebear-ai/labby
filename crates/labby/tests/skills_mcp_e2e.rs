//! Exercise native Skills through real Labby servers and production clients.
#![cfg(all(feature = "gateway", feature = "skills"))]
#![allow(dead_code, clippy::panic)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/skills_oauth.rs"]
mod skills_oauth;

use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use labby_runtime::skills::wire::{SkillsGetResult, SkillsListResult};
use rmcp::model::{ClientRequest, CustomRequest, ProtocolVersion, ReadResourceRequestParams};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use serde_json::json;
use std::time::Duration;

async fn verify_http_skills(endpoint: &str, expected_uri: &str) {
    for lifecycle in [
        ClientLifecycleMode::Initialize,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    ] {
        verify_http_skills_with_lifecycle(endpoint, expected_uri, lifecycle, false).await;
    }
}

async fn verify_http_skills_with_lifecycle(
    endpoint: &str,
    expected_uri: &str,
    lifecycle: ClientLifecycleMode,
    oauth: bool,
) {
    drop(rustls::crypto::ring::default_provider().install_default());
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.to_owned());
    config.auth_header = Some("skills-e2e-disposable-token".into());
    let capped = BodyCappedHttpClient::new(reqwest::Client::new(), 1024 * 1024);
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let client = if oauth {
            use rmcp::transport::auth::{AuthClient, AuthorizationManager};
            let manager = AuthorizationManager::new(endpoint)
                .await
                .expect("OAuth manager");
            let worker = StreamableHttpClientWorker::new(AuthClient::new(capped, manager), config);
            ().serve_with_lifecycle(worker, lifecycle).await
        } else {
            let worker = StreamableHttpClientWorker::new(capped, config);
            ().serve_with_lifecycle(worker, lifecycle).await
        }
        .expect("MCP connection");
        verify_skill_roundtrip(&client, expected_uri).await;
        client.cancel().await.expect("client shutdown");
    })
    .await;
    result.expect("Skills round trip deadline");
}

#[tokio::test]
async fn skills_oauth_wrapped_http_client_preserves_native_results() {
    let server = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .start()
        .await
        .expect("isolated Labby starts");
    verify_http_skills_with_lifecycle(
        &format!("{}/mcp", server.connection().base_url),
        "skill://labby/using-labby/SKILL.md",
        ClientLifecycleMode::Initialize,
        true,
    )
    .await;
    let cleanup = server.finish().await;
    assert!(cleanup.is_clean(), "owned server cleanup: {cleanup:?}");
}

async fn verify_skill_roundtrip(
    client: &rmcp::service::Peer<rmcp::RoleClient>,
    expected_uri: &str,
) {
    let list: SkillsListResult = client
        .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/list",
            Some(json!({})),
        )))
        .await
        .expect("native skills/list");
    let skill = list
        .skills
        .iter()
        .find(|skill| skill.uri == expected_uri)
        .unwrap_or_else(|| panic!("expected {expected_uri} in {list:?}"));
    let get: SkillsGetResult = client
        .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/get",
            Some(json!({"uri": skill.uri})),
        )))
        .await
        .expect("native skills/get");
    assert_eq!(get.skill.uri, skill.uri);
    let read = client
        .read_resource(ReadResourceRequestParams::new(skill.uri.clone()))
        .await
        .expect("manifest-bound SKILL.md read");
    let [rmcp::model::ResourceContents::TextResourceContents { text, uri, .. }] =
        read.contents.as_slice()
    else {
        panic!("SKILL.md must return exactly one text resource");
    };
    assert_eq!(uri, expected_uri);
    assert!(text.contains("name: using-labby"));
    let manifest = get.skill.resources.as_ref().expect("stable manifest");
    let file = manifest
        .iter()
        .find(|file| file.uri == expected_uri)
        .expect("entrypoint binding");
    use sha2::{Digest as _, Sha256};
    assert_eq!(
        file.digest,
        format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())))
    );
    assert_eq!(file.size, text.len() as u64);
    for file in manifest.iter().filter(|file| file.uri != expected_uri) {
        let read = client
            .read_resource(ReadResourceRequestParams::new(file.uri.clone()))
            .await
            .expect("supporting manifest resource read");
        let [content] = read.contents.as_slice() else {
            panic!("each manifest file must return exactly one resource");
        };
        let (uri, bytes) = match content {
            rmcp::model::ResourceContents::TextResourceContents { uri, text, .. } => {
                (uri, text.as_bytes().to_vec())
            }
            rmcp::model::ResourceContents::BlobResourceContents { uri, blob, .. } => {
                use base64::Engine as _;
                (
                    uri,
                    base64::engine::general_purpose::STANDARD
                        .decode(blob)
                        .expect("valid resource base64"),
                )
            }
            _ => panic!("unsupported manifest resource representation"),
        };
        assert_eq!(uri, &file.uri);
        assert_eq!(file.size, bytes.len() as u64);
        assert_eq!(
            file.digest,
            format!("sha256:{}", hex::encode(Sha256::digest(&bytes)))
        );
    }
    let unknown = client
        .send_request_as::<SkillsGetResult>(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/get",
            Some(json!({"uri": "skill://labby/missing/SKILL.md"})),
        )))
        .await
        .expect_err("unknown skill must be rejected");
    assert!(
        matches!(unknown, rmcp::service::ServiceError::McpError(error)
            if error.code == rmcp::model::ErrorCode::INVALID_PARAMS)
    );
}

#[tokio::test]
async fn skills_stdio_server_list_get_and_read() {
    for lifecycle in [
        ClientLifecycleMode::Initialize,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    ] {
        let home = tempfile::tempdir().expect("isolated home");
        let mut command = live_labby::isolated_command(home.path());
        command.arg("mcp");
        let transport =
            rmcp::transport::TokioChildProcess::new(tokio::process::Command::from(command))
                .expect("stdio transport");
        tokio::time::timeout(Duration::from_secs(30), async {
            let client =
                ().serve_with_lifecycle(transport, lifecycle)
                    .await
                    .expect("stdio initialize");
            verify_skill_roundtrip(&client, "skill://labby/using-labby/SKILL.md").await;
            client.cancel().await.expect("stdio shutdown");
        })
        .await
        .expect("stdio Skills deadline");
    }
}

#[tokio::test]
async fn skills_http_server_list_get_and_read_through_production_client() {
    let server = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .start()
        .await
        .expect("isolated Labby starts");
    verify_http_skills(
        &format!("{}/mcp", server.connection().base_url),
        "skill://labby/using-labby/SKILL.md",
    )
    .await;
    let cleanup = server.finish().await;
    assert!(cleanup.is_clean(), "owned server cleanup: {cleanup:?}");
}

#[tokio::test]
async fn skills_http_gateway_federates_a_real_labby_server() {
    let leaf = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .start()
        .await
        .expect("leaf Labby starts");
    let config = format!(
        r#"
[[upstream]]
name = "skills-leaf"
enabled = true
url = "{}/mcp"
bearer_token_env = "SKILLS_E2E_LEAF_TOKEN"
proxy_skills = true
"#,
        leaf.connection().base_url
    );
    let gateway = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .env("SKILLS_E2E_LEAF_TOKEN", "skills-e2e-disposable-token")
        .config(config)
        .start()
        .await
        .expect("gateway Labby starts");
    verify_http_skills(
        &format!("{}/mcp", gateway.connection().base_url),
        "skill://skills-leaf/skill/labby/using-labby/SKILL.md",
    )
    .await;
    let gateway_cleanup = gateway.finish().await;
    let leaf_cleanup = leaf.finish().await;
    assert!(
        gateway_cleanup.is_clean(),
        "gateway cleanup: {gateway_cleanup:?}"
    );
    assert!(leaf_cleanup.is_clean(), "leaf cleanup: {leaf_cleanup:?}");
}

#[tokio::test]
#[cfg(unix)]
async fn skills_gateway_federates_a_stdio_labby_server() {
    let home = tempfile::tempdir().expect("isolated stdio home");
    std::fs::create_dir(home.path().join("tmp")).expect("stdio temporary directory");
    let command = live_labby::isolated_command(home.path());
    // Upstream config intentionally forbids HOME/LABBY_* overrides. This
    // disposable gateway explicitly trusts one launcher to give the leaf its
    // own installation and prevent attachment to an operator's running daemon.
    let mut args = vec!["-i".to_owned()];
    args.extend(command.get_envs().filter_map(|(key, value)| {
        value.map(|value| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
    }));
    args.push(command.get_program().to_string_lossy().into_owned());
    args.push("mcp".to_owned());
    let config = format!(
        "[gateway]\nextra_stdio_commands = [\"/usr/bin/env\"]\n[[upstream]]\nname = \"skills-stdio\"\ncommand = \"/usr/bin/env\"\nargs = {}\nproxy_skills = true\n",
        serde_json::to_string(&args).expect("isolated command arguments"),
    );
    let gateway = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .config(config)
        .start()
        .await
        .expect("stdio gateway starts");
    verify_http_skills(
        &format!("{}/mcp", gateway.connection().base_url),
        "skill://skills-stdio/skill/labby/using-labby/SKILL.md",
    )
    .await;
    let cleanup = gateway.finish().await;
    assert!(cleanup.is_clean(), "stdio gateway cleanup: {cleanup:?}");
}

#[tokio::test]
async fn skills_gateway_trust_policy_blocks_listing_get_and_resource_reads() {
    let leaf = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .start()
        .await
        .expect("leaf starts");
    for policy in [
        "proxy_skills = false",
        "proxy_skills = true\nexpose_skills = []",
        "proxy_skills = true\nexpose_skills = [\"not-using-labby\"]",
    ] {
        let config = format!(
            "[[upstream]]\nname = \"skills-leaf\"\nurl = \"{}/mcp\"\nbearer_token_env = \"SKILLS_E2E_LEAF_TOKEN\"\n{policy}\n",
            leaf.connection().base_url,
        );
        let gateway = live_labby::LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
            .env("SKILLS_E2E_LEAF_TOKEN", "skills-e2e-disposable-token")
            .config(config)
            .start()
            .await
            .expect("policy gateway starts");
        let endpoint = format!("{}/mcp", gateway.connection().base_url);
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        config.auth_header = Some("skills-e2e-disposable-token".into());
        let worker = StreamableHttpClientWorker::new(
            BodyCappedHttpClient::new(reqwest::Client::new(), 1024 * 1024),
            config,
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            let client =
                ().serve_with_lifecycle(worker, ClientLifecycleMode::Initialize)
                    .await
                    .expect("policy connection");
            let list: SkillsListResult = client
                .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
                    "skills/list",
                    Some(json!({})),
                )))
                .await
                .expect("policy list");
            assert!(
                list.skills
                    .iter()
                    .all(|skill| !skill.uri.starts_with("skill://skills-leaf/")),
                "hidden upstream leaked under {policy}: {list:?}"
            );
            // Prove the server remains functional; absence must not be caused
            // by a broken transport or a wholly empty Skills catalog.
            verify_skill_roundtrip(&client, "skill://labby/using-labby/SKILL.md").await;
            let uri = "skill://skills-leaf/skill/labby/using-labby/SKILL.md";
            let get = client
                .send_request_as::<SkillsGetResult>(ClientRequest::CustomRequest(
                    CustomRequest::new("skills/get", Some(json!({"uri": uri}))),
                ))
                .await
                .expect_err("hidden skill cannot be fetched directly");
            assert!(
                matches!(get, rmcp::service::ServiceError::McpError(_)),
                "expected protocol rejection under {policy}: {get:?}"
            );
            let read = client
                .read_resource(ReadResourceRequestParams::new(uri))
                .await
                .expect_err("hidden skill content cannot bypass policy through resources/read");
            assert!(
                matches!(read, rmcp::service::ServiceError::McpError(_)),
                "expected protocol rejection under {policy}: {read:?}"
            );
            client.cancel().await.expect("policy client shutdown");
        })
        .await
        .expect("policy deadline");
        let cleanup = gateway.finish().await;
        assert!(cleanup.is_clean(), "policy gateway cleanup: {cleanup:?}");
    }
    let cleanup = leaf.finish().await;
    assert!(cleanup.is_clean(), "policy leaf cleanup: {cleanup:?}");
}
