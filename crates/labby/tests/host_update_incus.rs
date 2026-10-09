#![cfg(unix)]
//! Exercise the real CLI and embedded release installer, replacing only the
//! external release download/provenance services and the Incus client.
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

const VERSION: &str = "v99.0.0";
const ASSET: &str = "lab-x86_64-unknown-linux-gnu.tar.gz";
const CANDIDATE: &str = "#!/bin/sh\nprintf 'labby 99.0.0\\n'\n";

struct Fixture {
    root: tempfile::TempDir,
    tools: PathBuf,
    install: PathBuf,
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        let release = root.path().join("release");
        let staging = root.path().join("staging");
        let install = root.path().join("install");
        for directory in [
            &tools,
            &release,
            &staging,
            &root.path().join("home"),
            &root.path().join("tmp"),
        ] {
            fs::create_dir(directory).unwrap();
        }
        // Never inherit the user's PATH: it could contain a real Incus client.
        // Keep the installer's filesystem, archive and checksum operations real.
        for tool in [
            "sh",
            "awk",
            "cat",
            "chmod",
            "cp",
            "date",
            "find",
            "gzip",
            "head",
            "install",
            "mkdir",
            "mktemp",
            "mv",
            "rm",
            "sed",
            "sort",
            "sync",
            "tar",
            "shasum",
            "sha256sum",
            "perl",
        ] {
            if let Some(source) = ["/usr/bin", "/bin"]
                .into_iter()
                .map(|directory| Path::new(directory).join(tool))
                .find(|path| path.is_file())
            {
                symlink(source, tools.join(tool)).unwrap();
            }
        }
        executable(&staging.join("labby"), CANDIDATE);
        let archive = release.join(ASSET);
        let status = std::process::Command::new(tools.join("tar"))
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .arg("labby")
            .status()
            .unwrap();
        assert!(status.success(), "fixture archive creation failed");
        let digest = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
        fs::write(
            release.join(format!("{ASSET}.sha256")),
            format!("{digest}  {ASSET}\n"),
        )
        .unwrap();
        fs::write(
            release.join(format!("{ASSET}.sigstore.jsonl")),
            "fixture-provenance\n",
        )
        .unwrap();
        executable(
            &tools.join("uname"),
            "#!/bin/sh\ncase \"$1\" in -s) echo Linux ;; -m) echo x86_64 ;; *) exit 64 ;; esac\n",
        );
        // Match only this fixture's immutable release URLs; an unexpected
        // request fails locally rather than reaching any real network tool.
        executable(
            &tools.join("curl"),
            r#"#!/bin/sh
out=
status=
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift 2 ;;
    -w) status=1; shift 2 ;;
    --proto|--proto-redir|--tlsv1.2|--max-redirs|--connect-timeout|--max-time|--retry|--max-filesize)
      if [ "$1" = --tlsv1.2 ]; then shift; else shift 2; fi ;;
    -fsSL) shift ;;
    https://github.com/dinglebear-ai/labby/releases/download/v99.0.0/*) url=$1; shift ;;
    *) exit 64 ;;
  esac
done
[ -n "$out" ] && [ -n "$url" ] || exit 64
name=${url##*/}
case "$name" in
  lab-x86_64-unknown-linux-gnu.tar.gz|lab-x86_64-unknown-linux-gnu.tar.gz.sha256|lab-x86_64-unknown-linux-gnu.tar.gz.sigstore.jsonl) ;;
  *) exit 64 ;;
esac
cp "$HOST_UPDATE_FIXTURE_RELEASE/$name" "$out" || exit 1
[ -z "$status" ] || printf 200
exit 0
"#,
        );
        executable(
            &tools.join("gh"),
            r#"#!/bin/sh
case "$*" in
  --version) echo 'gh version 2.102.0'; exit 0 ;;
  'attestation verify --help') exit 0 ;;
  'attestation verify '*)
    case "$*" in
      *'--bundle '*'--hostname github.com --repo dinglebear-ai/labby --signer-workflow dinglebear-ai/labby/.github/workflows/release.yml --source-ref refs/tags/v99.0.0 --deny-self-hosted-runners') exit 0 ;;
    esac ;;
esac
exit 64
"#,
        );
        Self {
            root,
            tools,
            install,
        }
    }

    async fn update(&self, cli_target: Option<&str>, env_target: Option<&str>) -> Output {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
        command
            .args([
                "--json",
                "host",
                "update",
                "--version",
                VERSION,
                "--no-web-assets",
                "--install-dir",
            ])
            .arg(&self.install)
            .env_clear()
            .env("PATH", &self.tools)
            .env("HOME", self.root.path().join("home"))
            .env("LABBY_HOME", self.root.path().join("home/.labby"))
            .env("TMPDIR", self.root.path().join("tmp"))
            .env(
                "HOST_UPDATE_FIXTURE_RELEASE",
                self.root.path().join("release"),
            )
            .env("NO_COLOR", "1")
            .current_dir(self.root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(target) = cli_target {
            command.args(["--container", target]);
        }
        if let Some(target) = env_target {
            command.env("LABBY_INCUS_CONTAINER", target);
        }
        tokio::time::timeout(Duration::from_secs(45), command.output())
            .await
            .expect("hermetic host update exceeded its deadline")
            .unwrap()
    }

    fn assert_installed(&self) {
        assert_eq!(
            fs::read_to_string(self.install.join("labby")).unwrap(),
            CANDIDATE,
            "negative control must reach Incus after completing the real installer"
        );
        let receipt = fs::read_to_string(self.install.join(".labby-install/receipt")).unwrap();
        assert!(receipt.contains("source=release\n"));
        assert!(receipt.contains("resolved_version=v99.0.0\n"));
        assert!(
            !self
                .install
                .join(".labby-install/transaction-lock")
                .exists()
        );
        assert!(
            !self
                .install
                .join(".labby-install/activation-journal")
                .exists()
        );
    }
}

fn assert_sync_error(output: &Output, expected: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "sync failure was hidden: {output:?}"
    );
    assert!(stderr.contains(expected), "wrong failure: {stderr}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("incus_sync_skipped"),
        "strict failure must not render a successful skipped update"
    );
}

#[tokio::test]
async fn host_update_without_incus_completes_install_and_reports_optional_skip() {
    let fixture = Fixture::new();
    let output = fixture.update(None, None).await;
    fixture.assert_installed();
    assert!(
        output.status.success(),
        "host update failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let outcome: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(outcome["installed"], true);
    assert_eq!(outcome["dry_run"], false);
    assert_eq!(outcome["incus_sync"], Value::Null);
    assert!(
        outcome["incus_sync_skipped"]
            .as_str()
            .unwrap()
            .contains("Incus client")
    );
    assert_eq!(
        Path::new(outcome["binary"].as_str().unwrap()),
        fixture.install.join("labby")
    );
}

#[tokio::test]
async fn host_update_without_incus_keeps_explicit_local_and_remote_targets_strict() {
    for target in ["labby", "remote:labby"] {
        let fixture = Fixture::new();
        let output = fixture.update(Some(target), None).await;
        fixture.assert_installed();
        assert_sync_error(&output, "Incus client");
    }
}

#[tokio::test]
async fn host_update_without_incus_keeps_environment_local_and_remote_targets_strict() {
    for target in ["labby", "remote:labby"] {
        let fixture = Fixture::new();
        let output = fixture.update(None, Some(target)).await;
        fixture.assert_installed();
        assert_sync_error(&output, "Incus client");
    }
}

#[tokio::test]
async fn host_update_keeps_incus_listing_failure_strict() {
    let fixture = Fixture::new();
    executable(
        &fixture.tools.join("incus"),
        "#!/bin/sh\nprintf 'permission denied by daemon' >&2\nexit 1\n",
    );
    let output = fixture.update(None, None).await;
    fixture.assert_installed();
    assert_sync_error(&output, "permission denied by daemon");
}

#[tokio::test]
async fn host_update_keeps_nonexecutable_incus_client_strict() {
    let fixture = Fixture::new();
    let client = fixture.tools.join("incus");
    fs::write(&client, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&client, fs::Permissions::from_mode(0o644)).unwrap();
    let output = fixture.update(None, None).await;
    fixture.assert_installed();
    assert_sync_error(&output, "failed to list Incus containers");
}

#[tokio::test]
async fn host_update_keeps_installed_incus_with_missing_interpreter_strict() {
    let fixture = Fixture::new();
    executable(
        &fixture.tools.join("incus"),
        &format!(
            "#!{}\n",
            fixture.root.path().join("absent-interpreter").display()
        ),
    );
    let output = fixture.update(None, None).await;
    fixture.assert_installed();
    assert_sync_error(&output, "failed to list Incus containers");
}
