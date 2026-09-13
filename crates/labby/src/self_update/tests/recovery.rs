use super::*;
use std::os::unix::fs::PermissionsExt as _;

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

// A prepared journal at the real installer's binary-publication boundary.
// Real interrupted installer coverage also lives in scripts/tests/installers.sh.
fn interrupted_install(directory: &Path) -> (std::path::PathBuf, Vec<u8>) {
    let journal = directory.join(".labby-install/activation-journal");
    fs::create_dir_all(&journal).unwrap();
    executable(&directory.join("labby"), "#!/bin/sh\necho 'labby 1.17.0'\n");
    executable(
        &journal.join("old-binary"),
        "#!/bin/sh\necho 'labby 1.16.0'\n",
    );
    fs::write(journal.join("old-binary.present"), "").unwrap();
    let receipt = b"resolved_version=v1.16.0\n".to_vec();
    fs::write(journal.join("old-receipt"), &receipt).unwrap();
    fs::write(journal.join("old-receipt.present"), "").unwrap();
    fs::write(directory.join(".labby-install/receipt"), &receipt).unwrap();
    fs::write(journal.join("state"), "binary-activated").unwrap();
    (journal, receipt)
}

#[tokio::test]
async fn recovery_precedes_version_execution_and_no_newer_catalog_decision() {
    let dir = tempfile::tempdir().unwrap();
    let (journal, receipt) = interrupted_install(dir.path());
    // An interrupted restoration can leave the executable itself unusable.
    // Recovery must precede even attempting --version.
    fs::write(dir.path().join("labby"), "incomplete executable").unwrap();
    let catalog = async {
        assert!(!journal.exists());
        assert_eq!(
            fs::read(dir.path().join(".labby-install/receipt")).unwrap(),
            receipt
        );
        assert!(acquire_update_lock(dir.path()).is_err());
        Ok(vec![Release {
            tag_name: "v1.16.0".into(),
            draft: false,
            prerelease: false,
            assets: vec![
                Asset { name: ASSET.into() },
                Asset {
                    name: format!("{ASSET}.sha256"),
                },
            ],
        }])
    };
    let result = automatic_with_catalog(&dir.path().join("labby"), false, catalog)
        .await
        .unwrap();
    assert_eq!(result["installed"], false);
    assert_eq!(result["reason"], "No newer stable Labby binary release");
    assert_eq!(
        Command::new(dir.path().join("labby"))
            .arg("--version")
            .output()
            .unwrap()
            .stdout,
        b"labby 1.16.0\n"
    );
    assert!(acquire_update_lock(dir.path()).is_ok());
}

#[tokio::test]
async fn dry_run_preserves_interrupted_activation_without_polling_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let (journal, _) = interrupted_install(dir.path());
    let binary = fs::read(dir.path().join("labby")).unwrap();
    let result = automatic_with_catalog(&dir.path().join("labby"), true, async {
        panic!("dry run must report required recovery before catalog access")
    })
    .await
    .unwrap();
    assert_eq!(result["recovery_required"], true);
    assert_eq!(result["installed"], false);
    assert_eq!(fs::read(dir.path().join("labby")).unwrap(), binary);
    assert_eq!(
        fs::read_to_string(journal.join("state")).unwrap(),
        "binary-activated"
    );
}

#[tokio::test]
async fn failed_recovery_retains_journal_and_never_polls_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let (journal, _) = interrupted_install(dir.path());
    fs::remove_file(journal.join("old-binary")).unwrap();
    let result = automatic_with_catalog(&dir.path().join("labby"), false, async {
        panic!("failed recovery must prevent catalog access")
    })
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("activation recovery FAILED")
    );
    assert!(journal.join("state").exists());
    assert!(acquire_update_lock(dir.path()).is_ok());
}

#[test]
fn schedule_status_child() {
    let Some(home) = std::env::var_os("LABBY_TEST_STATUS_HOME") else {
        return;
    };
    let result = schedule_for_paths(
        "status",
        false,
        Path::new(&home),
        Path::new("/opt/bin/labby"),
    )
    .unwrap();
    assert_eq!(result["enabled"], true);
}

#[test]
fn status_does_not_create_files_or_require_mutation_lock() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("tools");
    fs::create_dir(&fake).unwrap();
    executable(
        &fake.join("launchctl"),
        "#!/bin/sh\n[ \"$1\" = print ] || exit 99\nexit 0\n",
    );
    let home = dir.path().join("home");
    fs::create_dir(&home).unwrap();
    let run_status = || {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "self_update::tests::recovery::schedule_status_child",
                "--nocapture",
            ])
            .env("LABBY_TEST_STATUS_HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", fake.display()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    run_status();
    assert!(!home.join("Library").exists());
    let plist = home.join("Library/LaunchAgents/net.labby.auto-update.plist");
    let _lock = acquire_schedule_lock(&plist).unwrap();
    run_status();
    fs::set_permissions(plist.parent().unwrap(), fs::Permissions::from_mode(0o500)).unwrap();
    run_status();
    fs::set_permissions(plist.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
}
