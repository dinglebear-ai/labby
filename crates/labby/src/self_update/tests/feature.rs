//! Exercise automatic selection and the real installer without network or host state.
//! Only release catalog, downloads and attestation service are fixture boundaries.

use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::{fs::PermissionsExt, process::CommandExt};

const AUTOMATIC_FEATURE_CHILD_TIMEOUT: Duration = Duration::from_mins(1);

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
    original: Vec<u8>,
    original_receipt: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let base = if Path::new("/private/tmp").is_dir() {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        };
        let root = tempfile::tempdir_in(base).unwrap();
        for name in ["bin", "tools", "home", "tmp", "archive"] {
            fs::create_dir(root.path().join(name)).unwrap();
        }
        let original = b"#!/bin/sh\necho 'labby 1.16.0'\n".to_vec();
        executable(
            &root.path().join("bin/labby"),
            std::str::from_utf8(&original).unwrap(),
        );
        executable(
            &root.path().join("archive/labby"),
            "#!/bin/sh\necho 'labby 1.17.0'\n",
        );
        let archive = root.path().join(ASSET);
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(root.path().join("archive"))
            .arg("labby")
            .status()
            .unwrap();
        assert!(status.success());
        let digest = hex::encode(Sha256::digest(fs::read(archive).unwrap()));
        fs::write(
            root.path().join(format!("{ASSET}.sha256")),
            format!("{digest}  {ASSET}\n"),
        )
        .unwrap();
        executable(
            &root.path().join("tools/uname"),
            "#!/bin/sh\ncase \"$1\" in -s) echo Darwin;; -m) echo arm64;; *) exit 1;; esac\n",
        );
        executable(
            &root.path().join("tools/curl"),
            r#"#!/bin/sh
set -eu
out=
url=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift 2;;
    --connect-timeout|--max-time|--retry) shift 2;;
    -*) shift;;
    *) url=$1; shift;;
  esac
done
printf '%s\n' "$url" >> "$LABBY_TEST_FEATURE_ROOT/downloads"
case "$url" in
  https://github.com/dinglebear-ai/labby/releases/download/v1.17.0/lab-aarch64-apple-darwin.tar.gz*) ;;
  *) echo "unexpected network request: $url" >&2; exit 91;;
esac
[ -n "$out" ]
cp "$LABBY_TEST_FEATURE_ROOT/${url##*/}" "$out"
"#,
        );
        executable(
            &root.path().join("tools/gh"),
            r#"#!/bin/sh
set -eu
# The installer probes gh for attestation support and authentication before
# any download. Those probes are prerequisite checks, not attestation
# requests, so answer them without recording them.
case "$*" in
  "attestation verify --help"|"auth status --hostname github.com") exit 0;;
esac
printf '%s\n' "$*" >> "$LABBY_TEST_FEATURE_ROOT/attestations"
case "$*" in
  "attestation verify "*" --hostname github.com --repo dinglebear-ai/labby --signer-workflow dinglebear-ai/labby/.github/workflows/release.yml --source-ref refs/tags/v1.17.0 --deny-self-hosted-runners") ;;
  *) echo 'unexpected attestation request' >&2; exit 92;;
esac
[ "$LABBY_TEST_FEATURE_CASE" != attestation_failure ]
"#,
        );
        // Seed a committed local installation through the same real installer.
        let script = root.path().join("install.sh");
        fs::write(&script, INSTALL_SCRIPT).unwrap();
        let initial = Command::new("/bin/sh")
            .arg(script)
            .env_clear()
            .env("HOME", root.path().join("home"))
            .env("TMPDIR", root.path().join("tmp"))
            .env("PATH", "/usr/bin:/bin")
            .env("LABBY_INSTALL_DIR", root.path().join("bin"))
            .env("LABBY_INSTALL_LOCAL_BINARY", root.path().join("bin/labby"))
            .env(
                "LABBY_INSTALL_LOCAL_SHA256",
                hex::encode(Sha256::digest(&original)),
            )
            .env("LABBY_INSTALL_VERSION", "v1.16.0")
            .env("LABBY_INSTALL_NO_SETUP", "1")
            .output()
            .unwrap();
        assert!(
            initial.status.success(),
            "{}",
            String::from_utf8_lossy(&initial.stderr)
        );
        let original_receipt = fs::read(root.path().join("bin/.labby-install/receipt")).unwrap();
        Self {
            root,
            original,
            original_receipt,
        }
    }

    fn run(&self, case: &str) -> Value {
        let log = fs::File::create(self.root.path().join("child.log")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "self_update::tests::feature::automatic_feature_child",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", self.root.path().join("home"))
            .env("TMPDIR", self.root.path().join("tmp"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.path().join("tools").display()),
            )
            .env("LABBY_TEST_FEATURE_ROOT", self.root.path())
            .env("LABBY_TEST_FEATURE_CASE", case)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap();
        struct Cleanup(nix::unistd::Pid);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = nix::sys::signal::killpg(self.0, nix::sys::signal::Signal::SIGKILL);
            }
        }
        let _cleanup = Cleanup(nix::unistd::Pid::from_raw(
            i32::try_from(child.id()).unwrap(),
        ));
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > AUTOMATIC_FEATURE_CHILD_TIMEOUT {
                drop(child.kill());
                drop(child.wait());
                panic!(
                    "automatic feature child timed out: {}",
                    fs::read_to_string(self.root.path().join("child.log")).unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "{}",
            fs::read_to_string(self.root.path().join("child.log")).unwrap()
        );
        serde_json::from_slice(&fs::read(self.root.path().join("result.json")).unwrap()).unwrap()
    }

    fn assert_unchanged(&self) {
        assert_eq!(
            fs::read(self.root.path().join("bin/labby")).unwrap(),
            self.original
        );
        assert_eq!(
            fs::read(self.root.path().join("bin/.labby-install/receipt")).unwrap(),
            self.original_receipt
        );
        assert!(
            !self
                .root
                .path()
                .join("bin/.labby-install/previous-receipt")
                .exists()
        );
        assert!(
            !self
                .root
                .path()
                .join("bin/.labby-install/activation-journal")
                .exists()
        );
    }
}

#[tokio::test]
async fn automatic_feature_child() {
    let Some(root) = std::env::var_os("LABBY_TEST_FEATURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("LABBY_TEST_FEATURE_CASE").unwrap();
    let catalog = vec![release(if case == "no_newer" {
        "v1.16.0"
    } else {
        "v1.17.0"
    })];
    let result = automatic_with_catalog(&root.join("bin/labby"), case == "dry_run", async {
        Ok(catalog)
    })
    .await;
    let outcome = match result {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    fs::write(
        root.join("result.json"),
        serde_json::to_vec(&outcome).unwrap(),
    )
    .unwrap();
}

#[test]
fn automatic_installs_verified_newer_release_and_records_receipt() {
    let fixture = Fixture::new();
    let result = fixture.run("success");
    assert_eq!(result["ok"]["installed"], true);
    assert_eq!(result["ok"]["version"], "v1.17.0");
    let binary = fixture.root.path().join("bin/labby");
    assert_eq!(
        Command::new(&binary)
            .arg("--version")
            .output()
            .unwrap()
            .stdout,
        b"labby 1.17.0\n"
    );
    let receipt =
        fs::read_to_string(fixture.root.path().join("bin/.labby-install/receipt")).unwrap();
    assert!(receipt.contains("source=release\n"));
    assert!(receipt.contains("resolved_version=v1.17.0\n"));
    assert_eq!(
        fs::read(
            fixture
                .root
                .path()
                .join("bin/.labby-install/previous-receipt")
        )
        .unwrap(),
        fixture.original_receipt
    );
    assert!(receipt.contains(&format!(
        "sha256={}\n",
        hex::encode(Sha256::digest(fs::read(binary).unwrap()))
    )));
    assert!(
        !fixture
            .root
            .path()
            .join("bin/.labby-install/activation-journal")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("downloads"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("attestations"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn automatic_equal_release_and_dry_run_preserve_installation_without_downloads() {
    for case in ["no_newer", "dry_run"] {
        let fixture = Fixture::new();
        let result = fixture.run(case);
        assert_eq!(result["ok"]["installed"], false, "{result}");
        if case == "dry_run" {
            assert_eq!(result["ok"]["version"], "v1.17.0");
            assert_eq!(result["ok"]["dry_run"], true);
        } else {
            assert_eq!(
                result["ok"]["reason"],
                "No newer stable Labby binary release"
            );
        }
        fixture.assert_unchanged();
        assert!(!fixture.root.path().join("downloads").exists());
        assert!(!fixture.root.path().join("attestations").exists());
    }
}

#[test]
fn automatic_rejected_attestation_or_checksum_preserves_installed_binary() {
    for case in ["attestation_failure", "checksum_failure"] {
        let fixture = Fixture::new();
        if case == "checksum_failure" {
            fs::write(
                fixture.root.path().join(format!("{ASSET}.sha256")),
                format!("{}  {ASSET}\n", "0".repeat(64)),
            )
            .unwrap();
        }
        let result = fixture.run(case);
        let error = result["error"].as_str().expect("verification must fail");
        assert!(
            error.contains(if case == "checksum_failure" {
                "checksum verification FAILED"
            } else {
                "provenance verification FAILED"
            }),
            "{error}"
        );
        fixture.assert_unchanged();
        if case == "checksum_failure" {
            assert!(!fixture.root.path().join("attestations").exists());
        }
    }
}
