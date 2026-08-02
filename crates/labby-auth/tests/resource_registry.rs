use std::time::{Duration, SystemTime, UNIX_EPOCH};

use labby_auth::resource_registry::{ResourceRegistry, ResourceRegistryError};

fn unix_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn configured_resource_survives_lease_lifecycle_and_pruning() {
    let registry = ResourceRegistry::new();
    registry
        .replace_configured_resource_scopes([(
            "https://configured.example:8443/mcp/".to_string(),
            vec!["mcp:read".to_string()],
        )])
        .unwrap();

    let lease = registry
        .create_resource_lease_at(
            "https://leased.example:53147/mcp",
            ["mcp:read", "mcp:write"],
            Duration::from_mins(2),
            "proxy-run-1",
            unix_time(4_000_000_000),
        )
        .unwrap();
    registry
        .renew_resource_lease_at(&lease.id, Duration::from_mins(4), unix_time(4_000_000_030))
        .unwrap();
    registry.release_resource_lease(&lease.id).unwrap();
    registry.prune_expired_resource_leases(unix_time(2_000));

    assert_eq!(
        registry.effective_resource_scopes("https://configured.example:8443/mcp/"),
        Some(vec!["mcp:read".to_string()])
    );
}

#[test]
fn lease_survives_configured_resource_replacement() {
    let registry = ResourceRegistry::new();
    let lease = registry
        .create_resource_lease_at(
            "https://proxy.example:53147/mcp",
            ["mcp:read"],
            Duration::from_mins(2),
            "proxy-run-1",
            unix_time(4_000_000_000),
        )
        .unwrap();

    registry
        .replace_configured_resource_scopes([(
            "https://configured.example/mcp".to_string(),
            vec!["lab:admin".to_string()],
        )])
        .unwrap();

    assert_eq!(
        registry.effective_resource_scopes_at(
            "https://proxy.example:53147/mcp",
            unix_time(4_000_000_001)
        ),
        Some(vec!["mcp:read".to_string()])
    );
    assert_eq!(registry.lease_count(), 1);
    assert_eq!(registry.lease_diagnostics().len(), 1);
    assert!(!format!("{:?}", registry.lease_diagnostics()).contains(&lease.id));
}

#[test]
fn expiry_renewal_release_and_random_ids_are_enforced() {
    let registry = ResourceRegistry::new();
    let first = registry
        .create_resource_lease_at(
            "https://proxy.example:53147/mcp",
            ["mcp:read"],
            Duration::from_secs(10),
            "proxy-run-1",
            unix_time(1_000),
        )
        .unwrap();
    let second = registry
        .create_resource_lease_at(
            "https://proxy.example:53148/mcp",
            ["mcp:read"],
            Duration::from_secs(10),
            "proxy-run-2",
            unix_time(1_000),
        )
        .unwrap();
    assert_ne!(first.id, second.id);
    assert!(!format!("{first:?}").contains(&first.id));
    assert_eq!(first.expires_at_unix, 1_010);

    let renewed = registry
        .renew_resource_lease_at(&first.id, Duration::from_secs(30), unix_time(1_005))
        .unwrap();
    assert_eq!(renewed.expires_at_unix, 1_035);
    assert!(
        registry
            .effective_resource_scopes_at(&first.resource, unix_time(1_034))
            .is_some()
    );
    assert!(
        registry
            .effective_resource_scopes_at(&first.resource, unix_time(1_035))
            .is_none()
    );
    assert!(matches!(
        registry.renew_resource_lease_at(&first.id, Duration::from_secs(10), unix_time(1_035)),
        Err(ResourceRegistryError::LeaseNotFound)
    ));
    assert!(matches!(
        registry.release_resource_lease(&first.id),
        Err(ResourceRegistryError::LeaseNotFound)
    ));
}

#[test]
fn forced_process_style_drop_relies_on_expiry_and_restart_can_reregister_resource() {
    let registry = ResourceRegistry::new();
    let abandoned = registry
        .create_resource_lease_at(
            "https://proxy.example:53147/mcp",
            ["mcp:read"],
            Duration::from_secs(10),
            "process-before-restart",
            unix_time(1_000),
        )
        .unwrap();
    assert!(
        registry
            .effective_resource_scopes_at(&abandoned.resource, unix_time(1_009))
            .is_some()
    );
    assert!(
        registry
            .effective_resource_scopes_at(&abandoned.resource, unix_time(1_010))
            .is_none()
    );

    let restarted = registry
        .create_resource_lease_at(
            "https://proxy.example:53147/mcp",
            ["mcp:read", "mcp:write"],
            Duration::from_secs(20),
            "process-after-restart",
            unix_time(1_011),
        )
        .unwrap();
    assert_ne!(abandoned.id, restarted.id);
    assert_eq!(
        registry.effective_resource_scopes_at(&restarted.resource, unix_time(1_012)),
        Some(vec!["mcp:read".to_string(), "mcp:write".to_string()])
    );
}

#[test]
fn invalid_resource_scope_ttl_and_owner_are_rejected() {
    let registry = ResourceRegistry::new();
    let cases = [
        ("", vec!["mcp:read"], 10, "owner"),
        ("http://proxy.example/mcp", vec!["mcp:read"], 10, "owner"),
        ("relative/mcp", vec!["mcp:read"], 10, "owner"),
        (
            "https://proxy.example/mcp?x=1",
            vec!["mcp:read"],
            10,
            "owner",
        ),
        ("https://proxy.example/mcp", vec![], 10, "owner"),
        ("https://proxy.example/mcp", vec!["bad scope"], 10, "owner"),
        ("https://proxy.example/mcp", vec!["mcp:read"], 0, "owner"),
        (
            "https://proxy.example/mcp",
            vec!["mcp:read"],
            86_401,
            "owner",
        ),
        ("https://proxy.example/mcp", vec!["mcp:read"], 10, ""),
    ];

    for (resource, scopes, ttl_secs, owner) in cases {
        assert!(
            registry
                .create_resource_lease_at(
                    resource,
                    scopes,
                    Duration::from_secs(ttl_secs),
                    owner,
                    unix_time(1_000),
                )
                .is_err(),
            "case should be invalid: {resource:?} {owner:?}"
        );
    }
}

#[test]
fn canonicalization_preserves_exact_port_and_path_and_removes_one_slash() {
    let registry = ResourceRegistry::new();
    registry
        .replace_configured_resource_scopes([(
            "  https://proxy.example:53147/mcp/  ".to_string(),
            vec![" mcp:read ".to_string()],
        )])
        .unwrap();

    assert_eq!(
        registry.effective_resource_scopes("https://proxy.example:53147/mcp/"),
        Some(vec!["mcp:read".to_string()])
    );
    assert!(
        registry
            .effective_resource_scopes("https://proxy.example:53148/mcp/")
            .is_none()
    );
    assert!(
        registry
            .effective_resource_scopes("https://proxy.example:53147/other/")
            .is_none()
    );
}
