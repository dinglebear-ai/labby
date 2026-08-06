use std::collections::HashMap;
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under crates/labby")
        .to_path_buf()
}

fn classify(event: &str, files: &[&str]) -> HashMap<String, String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "lab-ci-paths-{}-{}-{}",
        std::process::id(),
        files.len(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos()
    ));
    drop(fs::remove_dir_all(&temp_dir));
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let changed = temp_dir.join("changed.txt");
    let output = temp_dir.join("github_output.txt");
    fs::write(&changed, files.join("\n")).expect("write changed file list");

    let status = Command::new("python3")
        .arg(repo_root().join("scripts/ci/changed_paths.py"))
        .arg("--event")
        .arg(event)
        .arg("--changed-files")
        .arg(&changed)
        .arg("--output")
        .arg(&output)
        .stdout(Stdio::null())
        .status()
        .expect("run changed_paths.py");
    assert!(status.success(), "changed_paths.py exited with {status}");

    let raw = fs::read_to_string(&output).expect("read github output");
    raw.lines()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key=value output");
            (key.to_string(), value.to_string())
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    let mut permissions = fs::metadata(path)
        .expect("read fake executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fake executable runnable");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_build_preflight_accepts_installed_libxdo_without_pkg_config_metadata() {
    let action =
        fs::read_to_string(repo_root().join(".github/actions/setup-rust-kache/action.yml"))
            .expect("read setup-rust-kache action");
    let action: serde_yaml::Value = serde_yaml::from_str(&action).expect("parse composite action");
    let preflight = action["runs"]["steps"]
        .as_sequence()
        .and_then(|steps| steps.first())
        .and_then(|step| step["run"].as_str())
        .expect("first action step has a shell preflight");

    let temp = tempfile::tempdir().expect("create fake command directory");
    let fake_bin = temp.path().join("bin");
    fs::create_dir(&fake_bin).expect("create fake bin");
    for command in ["cc", "ld.lld"] {
        write_executable(&fake_bin.join(command), "#!/bin/sh\nexit 0\n");
    }
    write_executable(
        &fake_bin.join("pkg-config"),
        concat!(
            "#!/bin/sh\n[ \"$",
            "{",
            "2:-",
            "}",
            "\" = xdo ] && exit 1\nexit 0\n"
        ),
    );
    write_executable(
        &fake_bin.join("dpkg-query"),
        "#!/bin/sh\n: > \"$DPKG_MARKER\"\nprintf 'ii '\n",
    );
    write_executable(&fake_bin.join("id"), "#!/bin/sh\nprintf '0\\n'\n");
    write_executable(
        &fake_bin.join("apt-get"),
        "#!/bin/sh\n: > \"$APT_MARKER\"\nexit 0\n",
    );

    let apt_marker = temp.path().join("apt-ran");
    let dpkg_marker = temp.path().join("dpkg-queried");
    let status = Command::new("bash")
        .arg("-c")
        .arg(preflight)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("APT_MARKER", &apt_marker)
        .env("DPKG_MARKER", &dpkg_marker)
        .status()
        .expect("run Linux prerequisite preflight");

    assert!(status.success(), "prerequisite preflight must succeed");
    assert!(
        dpkg_marker.exists(),
        "libxdo-dev must be checked through Debian package metadata"
    );
    assert!(
        !apt_marker.exists(),
        "an installed libxdo-dev package must not trigger apt-get just because xdo.pc is absent"
    );
}

#[test]
fn docs_only_changes_skip_expensive_runtime_categories() {
    let out = classify(
        "pull_request",
        &[
            "docs/runtime/CICD.md",
            "docs/sessions/2026-06-27-example.md",
        ],
    );
    assert_eq!(out["docs"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["web"], "false");
    assert_eq!(out["npm"], "false");
    assert_eq!(out["docker"], "false");
    assert_eq!(out["security"], "false");
    assert_eq!(out["release"], "false");
    // Prose docs cannot invalidate generated artifacts, so they must not
    // trigger the docs-check build either.
    assert_eq!(out["docs_check"], "false");
}

#[test]
fn npm_launcher_changes_enable_npm_checks_only() {
    let out = classify("pull_request", &["packages/labby-mcp/lib/platform.js"]);
    assert_eq!(out["npm"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["web"], "false");
    assert_eq!(out["docker"], "false");
    assert_eq!(out["security"], "false");
}

#[test]
fn server_json_changes_enable_npm_registry_checks() {
    let out = classify("pull_request", &["server.json"]);
    assert_eq!(out["npm"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
}

#[test]
fn rust_changes_enable_compile_test_security_release_and_container_smoke() {
    let out = classify("pull_request", &["crates/labby/src/dispatch/gateway.rs"]);
    assert_eq!(out["rust_compile"], "true");
    assert_eq!(out["rust_test"], "true");
    assert_eq!(out["security"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["docker"], "true");
    assert_eq!(out["web"], "false");
}

#[test]
fn rust_manifests_lockfiles_and_toolchains_run_full_tests() {
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "build.rs",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["rust_compile"], "true", "{path}");
        assert_eq!(out["rust_test"], "true", "{path}");
        assert_eq!(out["release"], "true", "{path}");
    }
}

#[test]
fn frontend_changes_enable_web_release_and_container_without_rust_tests() {
    let out = classify("pull_request", &["apps/gateway-admin/app/page.tsx"]);
    assert_eq!(out["web"], "true");
    assert_eq!(out["release"], "true");
    assert_eq!(out["docker"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["security"], "false");
}

#[test]
fn explicit_policy_files_route_to_the_right_checks() {
    let actionlint = classify("pull_request", &[".github/actionlint.yaml"]);
    assert_eq!(actionlint["workflow"], "true");

    let deny = classify("pull_request", &["deny.toml"]);
    assert_eq!(deny["security"], "true");
    assert_eq!(deny["rust_compile"], "true");
    assert_eq!(deny["rust_test"], "true");

    let generated_doc = classify("pull_request", &["docs/generated/cli-help.md"]);
    assert_eq!(generated_doc["docs_check"], "true");
    assert_eq!(generated_doc["rust_compile"], "false");
    assert_eq!(generated_doc["rust_test"], "false");
}

#[test]
fn palette_changes_route_to_dedicated_checks() {
    let out = classify("pull_request", &["apps/palette-tauri/src/App.tsx"]);
    assert_eq!(out["palette"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["web"], "false");
}

#[test]
fn ci_workflow_and_action_changes_enable_everything() {
    for path in [
        ".github/workflows/ci.yml",
        ".github/actions/setup-rust-kache/action.yml",
        "scripts/ci/changed_paths.py",
    ] {
        let out = classify("pull_request", &[path]);
        for (key, value) in out {
            assert_eq!(value, "true", "{path} must enable {key}");
        }
    }
}

#[test]
fn secondary_workflow_changes_enable_only_their_own_categories() {
    // Non-ci.yml workflow files enable the workflow gate (actionlint,
    // mcp-conformance) without re-running the full Rust/web/palette suites.
    for path in [
        "conformance/expected-failures-dated.yaml",
        "conformance/expected-failures-extensions.yaml",
        ".github/labeler.yml",
        ".github/actionlint.yaml",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["workflow"], "true", "{path} must enable workflow");
        assert_eq!(out["all"], "false", "{path} must not force everything");
        assert_eq!(out["rust_compile"], "false", "{path}");
        assert_eq!(out["rust_test"], "false", "{path}");
        assert_eq!(out["web"], "false", "{path}");
        assert_eq!(out["palette"], "false", "{path}");
        assert_eq!(out["release"], "false", "{path}");
    }
}

#[test]
fn release_workflow_changes_enable_the_release_contract() {
    for path in [
        ".github/workflows/release.yml",
        ".github/workflows/build-incus-image.yml",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["workflow"], "true", "{path}");
        assert_eq!(out["release"], "true", "{path}");
        assert_eq!(out["rust_compile"], "false", "{path}");
    }
}

#[test]
fn unraid_plugin_changes_route_to_the_unraid_check() {
    for path in [
        "unraid/labby.plg",
        "unraid/source/usr/local/emhttp/plugins/labby/Labby.page",
        "scripts/ci/unraid-plugin-checksums.sh",
        "scripts/ci/unraid-runtime-tests.sh",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["unraid"], "true", "{path} must enable unraid");
        assert_eq!(out["rust_compile"], "false", "{path}");
        assert_eq!(out["rust_test"], "false", "{path}");
    }

    let out = classify("pull_request", &["docs/runtime/UNRAID.md"]);
    assert_eq!(
        out["unraid"], "false",
        "prose docs must not run the plugin check"
    );
}

#[test]
fn scheduled_and_manual_runs_enable_everything() {
    for event in ["schedule", "workflow_dispatch"] {
        let out = classify(event, &["docs/runtime/CICD.md"]);
        for (key, value) in out {
            assert_eq!(value, "true", "{key} should be true for {event}");
        }
    }
}

#[test]
fn ci_workflow_uses_changed_path_classifier_and_stable_gate() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("read ci.yml")
        .replace("\r\n", "\n");

    assert!(
        workflow.contains("  changes:"),
        "CI must define a changes job"
    );
    assert!(
        workflow.contains("scripts/ci/changed_paths.py"),
        "CI must run the changed-path classifier"
    );
    assert!(
        workflow.contains("needs.changes.outputs.rust_compile"),
        "CI jobs must use changed-path outputs"
    );
    assert!(
        workflow.contains("needs.changes.outputs.rust_test"),
        "full test jobs must be separately gated from compile jobs"
    );
    assert!(
        workflow.contains("needs.changes.outputs.docs_check"),
        "generated docs freshness must have an explicit routing category"
    );
    assert!(
        workflow.contains("  ci-gate:"),
        "CI must expose a stable aggregate ci-gate job"
    );
    assert!(
        workflow.contains("success|skipped"),
        "ci-gate must accept intentionally skipped jobs"
    );
    for required in [
        "gateway-admin-browser",
        "codemode-runner-smoke",
        "mcp-regressions",
        "palette-web",
        "palette-rust",
        "rust-coverage",
    ] {
        assert!(
            workflow.contains(&format!("- {required}"))
                && workflow.contains(&format!("needs.{required}.result")),
            "ci-gate must aggregate {required}"
        );
    }
    let gate = workflow
        .split("  ci-gate:")
        .nth(1)
        .expect("ci-gate job body");
    for advisory in ["test-windows", "palette-windows"] {
        assert!(
            workflow.contains(&format!("  {advisory}:")),
            "CI must retain the advisory {advisory} job"
        );
        assert!(
            !gate.contains(&format!("- {advisory}"))
                && !gate.contains(&format!("needs.{advisory}.result")),
            "ci-gate must not aggregate advisory job {advisory}"
        );
    }

    assert!(
        workflow.contains(
            "palette: ${{ steps.classify.outputs.palette == 'true' || steps.classify.outputs.all == 'true' }}"
        ),
        "Palette routing must compare output strings explicitly so `false` cannot mask fail-closed `all=true`"
    );
    let browser_job = workflow
        .split("  gateway-admin-browser:")
        .nth(1)
        .and_then(|section| section.split("\n  fmt:").next())
        .expect("Gateway Admin browser job");
    assert!(browser_job.contains("pnpm test:browser"));
    assert!(browser_job.contains("Install Playwright runtime libraries"));
    assert!(
        browser_job.contains("PLAYWRIGHT_BROWSERS_PATH: /home/runner/.cache/ms-playwright"),
        "Playwright must use the fleet-mounted browser cache regardless of runner UID"
    );
    for library in ["libasound2t64", "libgbm1", "libnss3", "libxkbcommon0"] {
        assert!(
            browser_job.contains(library),
            "Ubuntu 26.04 runners must install the Chromium runtime library {library}"
        );
    }
    assert!(
        !browser_job.contains("playwright install-deps"),
        "Ubuntu 26.04 runners must install explicit runtime libraries instead of using Playwright's unsupported distro detector"
    );
    assert!(browser_job.contains("Verify cached Playwright browser launch"));
    assert!(browser_job.contains("chromium.executablePath()"));
    assert!(browser_job.contains("fs.existsSync(executable)"));
    assert!(browser_job.contains("chromium.launch({ headless: true })"));
    assert!(
        !browser_job.contains("pnpm exec playwright install chromium"),
        "Ubuntu 26.04 runners must use the image-provided Playwright browser"
    );
    assert!(browser_job.contains("needs.changes.outputs.web == 'true'"));

    let codemode_smoke = workflow
        .split("  codemode-runner-smoke:")
        .nth(1)
        .and_then(|section| section.split("\n  npm-launcher:").next())
        .expect("Code Mode runner smoke job");
    assert!(
        codemode_smoke.contains("cargo run -p labby --bin labby --all-features --locked --"),
        "Code Mode smoke must select the public binary when test fixtures add more binaries"
    );

    let feature_slices = workflow
        .split("  feature-slices:\n")
        .nth(1)
        .and_then(|section| section.split("\n  extracted-crate-slices:").next())
        .expect("feature-slices job");
    assert!(
        feature_slices.contains("if: matrix.slice == 'fs'"),
        "the fs slice must execute its no-gateway regression in CI"
    );
    assert!(
        feature_slices.contains(
            "cargo test -p labby --no-default-features --features fs --locked --test doctor_proxy_preflight"
        ),
        "the fs slice must run the proxy preflight integration binary without gateway"
    );

    for (job, next_job) in [
        ("feature-slices", "extracted-crate-slices"),
        ("test", "test-fork"),
        ("test-fork", "test-windows"),
    ] {
        let section = workflow
            .split(&format!("  {job}:\n"))
            .nth(1)
            .and_then(|body| body.split(&format!("\n  {next_job}:")).next())
            .expect("memory-constrained Rust job body");
        assert!(
            section.contains("CARGO_BUILD_JOBS: \"1\""),
            "{job} must serialize Cargo builds below the shared pool memory limit"
        );
        assert!(
            section.contains("RUSTFLAGS: \"-C linker=clang -C link-arg=-fuse-ld=lld\""),
            "{job} must use the lower-memory lld linker"
        );
    }
}

#[test]
fn cargo_run_defaults_to_public_labby_binary() {
    let manifest = fs::read_to_string(repo_root().join("crates/labby/Cargo.toml"))
        .expect("read labby Cargo.toml");
    let manifest: toml::Value = toml::from_str(&manifest).expect("parse labby Cargo.toml");

    assert_eq!(
        manifest["package"]["default-run"].as_str(),
        Some("labby"),
        "unqualified `cargo run -p labby` must keep selecting the public CLI binary"
    );
}

#[test]
fn github_actions_are_immutable_sha_pinned() {
    let github = repo_root().join(".github");
    let mut pending = vec![github.join("workflows"), github.join("actions")];
    let mut violations = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).expect("read GitHub automation directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("yml" | "yaml")
            ) {
                continue;
            }
            for (line_number, line) in fs::read_to_string(&path)
                .expect("read workflow")
                .lines()
                .enumerate()
            {
                let Some((_, target)) = line.split_once("uses:") else {
                    continue;
                };
                let target = target
                    .split('#')
                    .next()
                    .expect("uses target")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                if target.starts_with("./") {
                    continue;
                }
                let pinned = target.rsplit_once('@').is_some_and(|(_, revision)| {
                    revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit())
                });
                if !pinned {
                    violations.push(format!("{}:{}: {target}", path.display(), line_number + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "mutable action references:\n{}",
        violations.join("\n")
    );
}

#[test]
fn draft_releases_are_surfaced_without_being_auto_published() {
    let reminder =
        fs::read_to_string(repo_root().join(".github/workflows/release-publish-reminder.yml"))
            .expect("read release publish reminder workflow");

    // The reminder exists because unpublished drafts ship no artifacts. It must
    // never resolve that by publishing one itself — approval stays manual.
    assert!(!reminder.contains("--draft=false"));
    assert!(!reminder.contains("updateRelease"));
    assert!(!reminder.contains("draft: false"));

    assert!(!reminder.contains("createRelease"));
    assert!(!reminder.contains("uploadReleaseAsset"));
    assert!(!reminder.contains("deleteRelease"));

    assert!(reminder.contains("issues: write"));
    assert!(reminder.contains("listReleases"));
    assert!(reminder.contains("schedule:"));

    // `contents: write` is load-bearing: GitHub returns draft releases only to
    // callers with push access, so a read-only token lists zero drafts and this
    // workflow degrades to a silent no-op. It shipped that way in #350 and did
    // nothing. The scope buys visibility; the assertions above are what keep it
    // from being used to publish.
    assert!(
        reminder.contains("contents: write"),
        "read-only token cannot see drafts; the reminder would silently do nothing"
    );
}

#[test]
fn release_tool_downloads_are_version_and_digest_pinned() {
    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(!release.contains("/latest/download/"));
    assert!(!release.contains("mcp-publisher"));
    assert!(!release.contains("registry.modelcontextprotocol.io"));
    let registry = fs::read_to_string(repo_root().join(".github/workflows/mcp-registry.yml"))
        .expect("read MCP Registry workflow");
    assert!(registry.contains("mcp-registry-publish.yml@befa67c7b7f976235bf3fbced6ede93293a7f405"));
    assert!(!registry.contains("auth-method:"));
    assert!(registry.contains("manifest-path: server.json"));
    assert!(registry.contains("MCP_PRIVATE_KEY"));
    let incus = fs::read_to_string(repo_root().join(".github/workflows/build-incus-image.yml"))
        .expect("read hosted Incus workflow");
    assert!(incus.contains("distrobuilder_version=3.3.1"));
    assert!(incus.contains(
        "distrobuilder_sha256=6c411af7178bb55ef649c708f4f38fc3c30e6ecce901c08d8a389448a900a73a"
    ));
    assert!(incus.contains("go build -mod=vendor -trimpath"));
    assert!(!incus.contains("snap install distrobuilder"));

    let config = fs::read_to_string(repo_root().join("release-please-config.json"))
        .expect("read release-please config");
    assert!(!config.contains("\"skip-github-release\": true"));
    assert!(config.contains("\"draft\": true"));
    assert!(config.contains("\"force-tag-creation\": true"));

    assert!(
        release.lines().collect::<Vec<_>>().windows(2).any(|lines| {
            lines[0].trim() == "release:" && lines[1].trim() == "types: [published]"
        })
    );
    assert!(release.contains("--json isDraft --jq .isDraft"));
    assert!(release.contains("gh release upload \"$RELEASE_TAG\" \"${files[@]}\" --clobber"));
    assert!(!release.contains("gh release edit \"${GITHUB_REF_NAME}\" --draft=false"));
    assert!(release.contains("if [[ -f /tmp/labby-new-version-image ]]"));
    assert!(release.contains("LABBY_RELEASE_ASSET_DIR: ${{ github.workspace }}"));
    assert!(release.contains("NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}"));
    let npm_identity = release
        .find("name: Validate npm publication identity")
        .expect("release must authenticate to npm before publication");
    let artifact_upload = release
        .find("name: Upload assets to the published release")
        .expect("release must upload assets to the published release");
    assert!(release.contains("run: npm whoami >/dev/null"));
    assert!(npm_identity < artifact_upload);
}

#[test]
fn rolling_incus_release_promotes_verified_immutable_assets_before_tag() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/build-incus-image.yml"))
        .expect("read Incus image workflow");
    let upload = workflow
        .find("gh release upload \"$ROLLING_TAG\" \"$verify_dir\"/* --clobber")
        .expect("rolling release must receive immutable release assets");
    let rolling_verify = workflow
        .find("cd \"$rolling_verify\" && sha256sum --check --strict")
        .expect("rolling assets must be downloaded and checksum-verified");
    let advance = workflow
        .find("git push -f")
        .expect("rolling tag must advance explicitly");
    assert!(
        upload < rolling_verify && rolling_verify < advance,
        "rolling assets must be uploaded and remotely verified before the tag advances"
    );
}

/// Regression guard for lab-26zqj.
///
/// `publish-image` has no source dependency, so it is tempting to leave the
/// checkout out. But `gh` resolves its target repository from git remotes,
/// so without one every `gh release` call aborts with "failed to run git:
/// fatal: not a git repository" — before uploading anything. That is how the
/// Incus image asset silently stopped shipping after v1.8.5: the image built,
/// the artifact checksum-verified, and then the first upload died, so every
/// tag from v1.8.6 onward published without
/// `labby-incus-x86_64-unknown-linux-gnu.tar.xz`. Nothing downstream noticed,
/// because the missing asset only surfaces when someone tries to pin a newer
/// `INCUS_IMAGE_VERSION`.
///
/// The ordering half matters just as much: `actions/checkout` cleans the
/// workspace by default, so a checkout placed *after* `download-artifact`
/// deletes `dist/` and breaks publication a second, different way.
#[test]
fn incus_publish_job_checks_out_before_downloading_artifacts() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/build-incus-image.yml"))
        .expect("read Incus image workflow");
    let publish = workflow
        .find("publish-image:")
        .expect("workflow must define the publish-image job");
    let publish_section = &workflow[publish..];

    let checkout = publish_section
        .find("uses: actions/checkout@")
        .expect("publish-image must check out: gh resolves the repo from git remotes, and the trailing `git tag`/`git push` needs a work tree");
    let download = publish_section
        .find("uses: actions/download-artifact@")
        .expect("publish-image must download the built image artifact");
    assert!(
        checkout < download,
        "checkout must precede download-artifact — checkout cleans the workspace and would delete dist/"
    );

    assert!(
        publish_section.contains("GH_REPO: ${{ github.repository }}"),
        "publish-image must pin GH_REPO so gh stays correct even if the checkout is reconfigured"
    );
}
