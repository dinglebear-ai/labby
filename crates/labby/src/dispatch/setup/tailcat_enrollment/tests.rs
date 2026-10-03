use super::*;
fn root() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}
fn request() -> EnrollmentRequest {
    EnrollmentRequest {
        project_id: "bootstrap-default".into(),
        idempotency_key: "same-operation-key-0001".into(),
    }
}
#[test]
fn native_file_is_private_and_replay_retains_the_same_credential() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_temp, root) = root();
    let custody = prepare_custody(
        &root,
        "owner",
        &request(),
        "https://labby.example/sandbox",
        [4; 32],
    )
    .unwrap();
    let file = custody.journal.file.as_ref().unwrap();
    assert_eq!(
        std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = secure_file::read_verified(file).unwrap();
    assert!(
        labby_primitives::product_credential::ProductCredential::parse(
            std::str::from_utf8(&bytes).unwrap()
        )
        .is_ok()
    );
    let id = custody.journal.credential_id.clone();
    let original = file.clone();
    drop(custody);
    let replay = prepare_custody(
        &root,
        "owner",
        &request(),
        "https://labby.example/sandbox",
        [4; 32],
    )
    .unwrap();
    assert_eq!(id, replay.journal.credential_id);
    assert_eq!(original, *replay.journal.file.as_ref().unwrap());
}
#[test]
fn tampered_and_unjournaled_files_are_preserved_for_native_recovery() {
    let (_temp, root) = root();
    let mut custody = prepare_custody(
        &root,
        "owner",
        &request(),
        "https://labby.example/sandbox",
        [4; 32],
    )
    .unwrap();
    let file = custody.journal.file.as_ref().unwrap().path.clone();
    custody.journal.file = None;
    custody.journal.state = State::Allocating;
    secure_file::replace_journal(
        &custody.path,
        &serde_json::to_vec(&custody.journal).unwrap(),
    )
    .unwrap();
    drop(custody);
    assert!(
        prepare_custody(
            &root,
            "owner",
            &request(),
            "https://labby.example/sandbox",
            [4; 32]
        )
        .is_err()
    );
    assert!(file.exists());
    let (_temp, root) = self::root();
    let custody = prepare_custody(
        &root,
        "owner",
        &request(),
        "https://labby.example/sandbox",
        [4; 32],
    )
    .unwrap();
    let file = custody.journal.file.as_ref().unwrap().path.clone();
    drop(custody);
    std::fs::write(&file, b"foreign-replacement").unwrap();
    assert!(
        prepare_custody(
            &root,
            "owner",
            &request(),
            "https://labby.example/sandbox",
            [4; 32]
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"foreign-replacement");
}
#[tokio::test]
async fn committed_sql_receipt_recovers_published_journal_without_reissuing() {
    let (_temp, root) = root();
    let paths = InstallationPaths::from_root(root.clone()).unwrap();
    let store = AccessStore::open(paths.access_db()).await.unwrap();
    let identity = labby_auth::VerifiedIdentity::external(
        labby_auth::Authenticator::BrowserSession,
        "https://accounts.google.com",
        "fixture-owner",
    )
    .unwrap();
    store
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
        )
        .await
        .unwrap();
    let suffix = hex::encode(Sha256::digest(b"bootstrap-default"));
    let name = format!("tailcat-{}", &suffix[..16]);
    store
        .assign_project_loadout(
            crate::access::AssignProjectLoadoutInput::new(
                identity.clone(),
                "bootstrap-default",
                name.clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let custody = prepare_custody(
        &root,
        &identity.safe_fingerprint(),
        &request(),
        "https://labby.example/sandbox",
        [4; 32],
    )
    .unwrap();
    let file = custody.journal.file.as_ref().unwrap().clone();
    store
        .enroll_tailcat_credential(TailcatEnrollmentInput {
            identity,
            installation_id: "native-test-installation".into(),
            project_id: custody.journal.project_id.clone(),
            loadout_id: name.clone(),
            route_id: name,
            resource: custody.journal.resource.clone(),
            policy_fingerprint: custody.journal.policy_fingerprint,
            credential_id: custody.journal.credential_id.clone(),
            credential_digest: custody.journal.credential_digest,
            request_digest: custody.journal.request_digest,
            idempotency_digest: custody.journal.idempotency_digest,
            now: labby_auth::util::now_unix(),
            expires_at: custody.journal.expires_at,
        })
        .await
        .unwrap();
    let path = custody.path.clone();
    drop(custody);
    reconcile_pending(&paths, &store).await.unwrap();
    let journal: Journal =
        serde_json::from_slice(&secure_file::read_private(&path).unwrap()).unwrap();
    assert!(journal.state == State::Committed);
    assert_eq!(journal.file.as_ref().unwrap(), &file);
    secure_file::verify_identity(&file).unwrap();
    reconcile_pending(&paths, &store).await.unwrap();
}
#[test]
fn request_rejects_identity_and_credential_material_and_errors_are_redacted() {
    for payload in [
        json!({"project_id":"p","idempotency_key":"operation-1234567","subject":"spoof"}),
        json!({"project_id":"p","idempotency_key":"operation-1234567","credential":"secret"}),
    ] {
        assert!(serde_json::from_value::<EnrollmentRequest>(payload).is_err());
    }
    let error = recovery_required().to_string();
    assert!(!error.contains("lby_pc_v1"));
    assert!(error.contains("native recovery"));
}

#[tokio::test]
async fn rejected_enrollment_capacity_does_not_create_an_unrecoverable_lock() {
    let (_temp, root) = root();
    let paths = InstallationPaths::from_root(root.clone()).unwrap();
    let store = AccessStore::open(paths.access_db()).await.unwrap();
    let folder = root.join("tailcat/enrollments");
    for i in 0..128 {
        let mut operation = request();
        operation.idempotency_key = format!("capacity-operation-{i:04}");
        drop(
            prepare_custody(
                &root,
                "owner",
                &operation,
                "https://labby.example/sandbox",
                [4; 32],
            )
            .unwrap(),
        );
    }
    assert!(
        prepare_custody(
            &root,
            "owner",
            &request(),
            "https://labby.example/sandbox",
            [4; 32]
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 256);
    assert!(
        prepare_custody(
            &root,
            "owner",
            &request(),
            "https://labby.example/sandbox",
            [4; 32]
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 256);
    // The valid published files remain recoverable; rejected admission cannot
    // make startup refuse this directory or mutate existing credential custody.
    reconcile_pending(&paths, &store).await.unwrap();
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 256);
}
