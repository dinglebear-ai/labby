use super::fixture::{DEADLINE, client_info};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleServer,
    model::{ClientRequest, ErrorCode, ProtocolVersion},
    transport::{IntoTransport, Transport},
};
use serde_json::{Value, json};
use tokio::time::timeout;

#[tokio::test]
async fn sdk_auto_discovers_first_and_falls_back_to_explicit_legacy_version() {
    for legacy in [false, true] {
        let (server_transport, client_transport) = tokio::io::duplex(16384);
        let mut wire = IntoTransport::<RoleServer, _, _>::into_transport(server_transport);
        let server = tokio::spawn(async move {
            let discover =
                serde_json::to_value(timeout(DEADLINE, wire.receive()).await.unwrap().unwrap())
                    .unwrap();
            assert_eq!(discover["method"], "server/discover");
            let response = if legacy {
                json!({"jsonrpc": "2.0", "id": discover["id"], "error": {"code": -32601, "message": "legacy fixture"}})
            } else {
                json!({"jsonrpc": "2.0", "id": discover["id"], "result": {"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {}, "ttlMs": 0, "cacheScope": "private"}})
            };
            wire.send(serde_json::from_value(response).unwrap())
                .await
                .unwrap();
            if legacy {
                let init =
                    serde_json::to_value(timeout(DEADLINE, wire.receive()).await.unwrap().unwrap())
                        .unwrap();
                assert_eq!(init["method"], "initialize");
                assert_eq!(init["params"]["protocolVersion"], "2025-11-25");
                wire.send(serde_json::from_value(json!({"jsonrpc": "2.0", "id": init["id"], "result": {"protocolVersion": "2025-11-25", "capabilities": {}, "serverInfo": {"name": "legacy-fixture", "version": "1"}}})).unwrap()).await.unwrap();
                let initialized =
                    serde_json::to_value(timeout(DEADLINE, wire.receive()).await.unwrap().unwrap())
                        .unwrap();
                assert_eq!(initialized["method"], "notifications/initialized");
            }
            let ping: Value =
                serde_json::to_value(timeout(DEADLINE, wire.receive()).await.unwrap().unwrap())
                    .unwrap();
            assert_eq!(ping["method"], "ping", "modern path must not initialize");
            if !legacy {
                assert_eq!(
                    ping["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                    "2026-07-28"
                );
            }
            wire.send(serde_json::from_value(json!({"jsonrpc": "2.0", "id": ping["id"], "result": if legacy { json!({}) } else { json!({"resultType": "complete"}) }})).unwrap()).await.unwrap();
            drop(timeout(DEADLINE, wire.receive()).await);
        });
        let client = timeout(
            DEADLINE,
            client_info().serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Auto {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    legacy_version: Some(ProtocolVersion::V_2025_11_25),
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            client.peer_info().unwrap().protocol_version,
            if legacy {
                ProtocolVersion::V_2025_11_25
            } else {
                ProtocolVersion::V_2026_07_28
            }
        );
        timeout(
            DEADLINE,
            client.send_request(ClientRequest::PingRequest(Default::default())),
        )
        .await
        .unwrap()
        .unwrap();
        client.cancel().await.unwrap();
        timeout(DEADLINE, server).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn sdk_auto_does_not_downgrade_modern_protocol_errors() {
    for code in [
        ErrorCode::HEADER_MISMATCH,
        ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
    ] {
        let (server_transport, client_transport) = tokio::io::duplex(16384);
        let mut wire = IntoTransport::<RoleServer, _, _>::into_transport(server_transport);
        let server = tokio::spawn(async move {
            let discover =
                serde_json::to_value(timeout(DEADLINE, wire.receive()).await.unwrap().unwrap())
                    .unwrap();
            assert_eq!(discover["method"], "server/discover");
            wire.send(serde_json::from_value(json!({"jsonrpc": "2.0", "id": discover["id"], "error": {"code": code, "message": "modern rejection"}})).unwrap()).await.unwrap();
            let next = timeout(DEADLINE, wire.receive()).await;
            assert!(
                !matches!(next, Ok(Some(_))),
                "must not downgrade after a modern protocol error"
            );
        });
        let result = timeout(
            DEADLINE,
            client_info().serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Auto {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    legacy_version: Some(ProtocolVersion::V_2025_11_25),
                },
            ),
        )
        .await
        .unwrap();
        assert!(
            result.is_err(),
            "modern {code:?} rejection must abort startup"
        );
        timeout(DEADLINE, server).await.unwrap().unwrap();
    }
}
