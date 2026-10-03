use labby_tailcat::{BridgeConfig, BridgeError};
use std::net::SocketAddr;

fn config() -> BridgeConfig {
    BridgeConfig {
        executable: "/missing/tailcat-bridge".into(),
        state_dir: "/private/tmp".into(),
        expected_sha256: [0; 32],
        target: "127.0.0.1:8765".parse().unwrap(),
        peer: format!("nodekey:{}", "a".repeat(64)),
        derp_map_url: "https://tailcat.dev/derpmap.json".into(),
    }
}

#[test]
fn rejects_unsafe_target_before_touching_executable() {
    for target in ["192.0.2.1:8765", "[::ffff:192.0.2.1]:8765", "127.0.0.1:0"] {
        let mut c = config();
        c.target = target.parse::<SocketAddr>().unwrap();
        assert!(matches!(c.validate(), Err(BridgeError::InvalidConfig)));
    }
}

#[test]
fn rejects_untrusted_inputs() {
    let mut c = config();
    c.executable = "tailcat".into();
    assert!(matches!(c.validate(), Err(BridgeError::InvalidConfig)));
    c = config();
    c.peer.clear();
    assert!(matches!(c.validate(), Err(BridgeError::InvalidConfig)));
    c = config();
    c.derp_map_url = "http://evil.test/map".into();
    assert!(matches!(c.validate(), Err(BridgeError::InvalidConfig)));
}

#[test]
fn debug_never_contains_peer_or_paths() {
    let c = config();
    let debug = format!("{c:?}");
    assert!(!debug.contains("nodekey:"));
    assert!(!debug.contains("/missing"));
}
