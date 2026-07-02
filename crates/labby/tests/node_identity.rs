#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
use labby::config::{LabConfig, NodePreferences, NodeRole, NodeRuntimeRole};
use labby::node::identity::{resolve_runtime_role, resolve_runtime_role_from_config};

#[test]
fn resolves_master_role_when_master_matches_local_hostname() {
    let resolved = resolve_runtime_role("controller", Some("controller")).unwrap();
    assert!(matches!(resolved.role, NodeRole::Master));
}

#[test]
fn resolves_non_master_role_when_master_differs_from_local_hostname() {
    let resolved = resolve_runtime_role("node-a", Some("controller")).unwrap();
    assert!(matches!(resolved.role, NodeRole::NonMaster));
    assert_eq!(resolved.master_host, "controller");
}

#[test]
fn defaults_first_device_to_master_when_master_is_missing() {
    let resolved = resolve_runtime_role("controller", None).unwrap();
    assert!(matches!(resolved.role, NodeRole::Master));
    assert_eq!(resolved.master_host, "controller");
}

#[test]
fn treats_short_hostname_and_fqdn_as_same_device() {
    let resolved = resolve_runtime_role("controller", Some("controller.tailnet.ts.net")).unwrap();
    assert!(matches!(resolved.role, NodeRole::Master));
}

#[test]
fn does_not_treat_ip_addresses_with_same_first_octet_as_same_device() {
    let resolved = resolve_runtime_role("100.64.0.1", Some("100.88.0.2")).unwrap();
    assert!(matches!(resolved.role, NodeRole::NonMaster));
}

// ── Role resolution tests: these validate the early-return gate in serve.rs ──
//
// A node (NonMaster) resolving its role is the condition that triggers the
// `run_node_mode` early return before `build_default_registry()` (or, in a
// build without the `nodes` feature, the startup rejection). `node::identity`
// is ALWAYS-ON, so this file is deliberately ungated and runs in every build
// shape — including the gateway-only feature slice exercised in CI.

#[test]
fn role_resolution_node_returns_non_master() {
    let resolved = resolve_runtime_role("worker-01", Some("controller")).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "expected NonMaster but got {:?}",
        resolved.role
    );
    assert_eq!(resolved.local_host, "worker-01");
    assert_eq!(resolved.master_host, "controller");
}

#[test]
fn role_resolution_controller_returns_master() {
    let resolved = resolve_runtime_role("controller", Some("controller")).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "expected Master but got {:?}",
        resolved.role
    );
}

#[test]
fn role_resolution_no_controller_defaults_to_master() {
    let resolved = resolve_runtime_role("any-host", None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "expected Master (no controller configured) but got {:?}",
        resolved.role
    );
}

#[test]
fn config_with_controller_makes_different_host_a_node() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    let resolved = resolve_runtime_role_from_config("worker-02", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "host different from configured controller should be NonMaster, got {:?}",
        resolved.role
    );
}

#[test]
fn config_with_controller_makes_same_host_the_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    let resolved = resolve_runtime_role_from_config("controller.lab", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "host matching configured controller should be Master, got {:?}",
        resolved.role
    );
}

#[test]
fn explicit_role_node_override_with_different_controller_is_non_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    // --role node with a different controller host resolves to NonMaster.
    let resolved =
        resolve_runtime_role_from_config("worker-04", &config, Some(NodeRuntimeRole::Node))
            .unwrap();
    assert!(
        matches!(resolved.role, NodeRole::NonMaster),
        "explicit --role node with different controller should be NonMaster, got {:?}",
        resolved.role
    );
}

#[test]
fn explicit_role_controller_override_forces_master() {
    let config = LabConfig {
        node: Some(NodePreferences {
            controller: Some("controller.lab".to_string()),
            log_retention_days: None,
            role: None,
        }),
        ..LabConfig::default()
    };
    // Even if the hostname differs, --role controller forces Master.
    let resolved =
        resolve_runtime_role_from_config("worker-03", &config, Some(NodeRuntimeRole::Controller))
            .unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "explicit --role controller should force Master, got {:?}",
        resolved.role
    );
}

#[test]
fn no_node_config_defaults_to_master() {
    let config = LabConfig::default();
    let resolved = resolve_runtime_role_from_config("any-host", &config, None).unwrap();
    assert!(
        matches!(resolved.role, NodeRole::Master),
        "no node config should default to Master, got {:?}",
        resolved.role
    );
}

#[test]
fn role_node_without_controller_host_returns_error() {
    // --role node with no [node].controller configured must fail
    let config = LabConfig::default();
    let result = resolve_runtime_role_from_config("somehost", &config, Some(NodeRuntimeRole::Node));
    assert!(
        result.is_err(),
        "expected error when --role node has no controller host"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("controller host") || msg.contains("[node].controller"),
        "error message should mention controller host config, got: {msg}"
    );
}

#[test]
fn config_role_node_without_controller_host_returns_error() {
    // [node].role = "node" but no [node].controller configured must fail
    let config = LabConfig {
        node: Some(NodePreferences {
            role: Some(NodeRuntimeRole::Node),
            controller: None,
            log_retention_days: None,
        }),
        ..LabConfig::default()
    };
    let result = resolve_runtime_role_from_config("somehost", &config, None);
    assert!(
        result.is_err(),
        "expected error when [node].role=node but no controller host"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("controller host") || msg.contains("[node].controller"),
        "error message should mention controller host config, got: {msg}"
    );
}

#[test]
fn role_node_with_controller_host_succeeds() {
    // Success case: verifies the error is ONLY about missing host, not the role itself
    let config = LabConfig {
        node: Some(NodePreferences {
            role: None,
            controller: Some("node-a".to_string()),
            log_retention_days: None,
        }),
        ..LabConfig::default()
    };
    let result = resolve_runtime_role_from_config("somehost", &config, Some(NodeRuntimeRole::Node));
    assert!(
        result.is_ok(),
        "should succeed when controller host is configured: {result:?}"
    );
}
