use super::*;

#[test]
fn sealed_bridge_bearer_requires_the_installation_key_and_namespace() {
    let key = MessageKey::from_test_bytes([7; 32]);
    let other = MessageKey::from_test_bytes([8; 32]);
    let installation = "a".repeat(64);
    let secret = "synthetic-bearer";
    let encoded = key.seal_bridge_bearer(&installation, secret).unwrap();
    assert_ne!(
        encoded,
        key.seal_bridge_bearer(&installation, secret).unwrap()
    );
    assert_eq!(
        &*key.open_bridge_bearer(&installation, &encoded).unwrap(),
        secret.as_bytes()
    );
    assert!(other.open_bridge_bearer(&installation, &encoded).is_err());
    assert!(key.open_bridge_bearer(&"b".repeat(64), &encoded).is_err());
    let mut corrupted = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    corrupted[BRIDGE_NONCE_BYTES] ^= 1;
    assert!(
        key.open_bridge_bearer(&installation, &URL_SAFE_NO_PAD.encode(corrupted))
            .is_err()
    );
}

#[test]
fn bridge_proof_cannot_be_replayed_or_rebound() {
    let key = MessageKey::from_test_bytes([3; 32]);
    let forgery = MessageKey::from_test_bytes([5; 32]);
    let installation = "1".repeat(64);
    let other_installation = "2".repeat(64);
    let owner = "3".repeat(64);
    let epoch = ulid::Ulid::new().to_string();
    let token = "synthetic-token-one";
    let now = 1_000_000;
    let request = BridgeProofRequest::new(&key, token, &installation, now).unwrap();
    assert!(request.verify(&key, token, &installation, now).is_ok());
    assert!(
        request
            .verify(&key, "different-token", &installation, now)
            .is_err()
    );
    assert!(
        request
            .verify(&key, token, &other_installation, now)
            .is_err()
    );
    assert!(
        request
            .verify(&key, token, &installation, now + 30_001)
            .is_err()
    );

    let result = BridgeProof::issue(&key, &request, token, &owner, &epoch, now).unwrap();
    assert!(result.verify(&key, &request, token, now).is_ok());
    assert!(
        result
            .verify(&key, &request, "different-token", now)
            .is_err()
    );
    assert!(result.verify(&key, &request, token, now + 5_001).is_err());
    let retry = BridgeProofRequest::new(&key, token, &installation, now).unwrap();
    assert_ne!(request.nonce, retry.nonce);
    assert!(result.verify(&key, &retry, token, now).is_err());

    let fake = BridgeProof::issue(&forgery, &request, token, &owner, &epoch, now).unwrap();
    assert!(fake.verify(&key, &request, token, now).is_err());
    let mut changed = result;
    changed.owner_key = "4".repeat(64);
    assert!(changed.verify(&key, &request, token, now).is_err());
}

#[test]
fn bridge_key_is_durable_private_and_distinguishes_static_token_rotation() {
    let temp = tempfile::tempdir().unwrap();
    let one = MessageKey::open_at(temp.path()).unwrap();
    let two = MessageKey::open_at(temp.path()).unwrap();
    assert_eq!(
        one.static_owner("synthetic-old"),
        two.static_owner("synthetic-old")
    );
    assert_ne!(
        one.static_owner("synthetic-old"),
        two.static_owner("synthetic-new")
    );
    let path = temp.path().join("agent-notifications/messaging.key");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(MessageKey::open_at(temp.path()).is_err());
    }
}
