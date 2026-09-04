use std::collections::{BTreeSet, HashMap};
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

#[test]
fn distributable_labby_feature_profiles_always_include_skills() {
    let manifest_text = fs::read_to_string(repo_root().join("crates/labby/Cargo.toml"))
        .expect("read Labby manifest");
    let manifest = toml::from_str::<toml::Value>(&manifest_text).expect("parse Labby manifest");
    let features = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .expect("Labby feature table");
    let members = |feature: &str| {
        features
            .get(feature)
            .and_then(toml::Value::as_array)
            .map_or([].as_slice(), Vec::as_slice)
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<BTreeSet<_>>()
    };

    assert!(members("gateway").contains("skills"));
    assert!(members("gateway-host").contains("gateway"));
    assert!(members("integrated-gateway").contains("gateway"));
    assert!(members("default").contains("gateway-host"));
    assert!(members("all").contains("skills"));
}

#[test]
fn rust_setup_uses_writable_per_job_homes_when_runner_globals_are_read_only() {
    let action =
        fs::read_to_string(repo_root().join(".github/actions/setup-rust-kache/action.yml"))
            .expect("read setup-rust-kache action");

    let fallback = action
        .split("- name: Select writable Rust homes")
        .nth(1)
        .and_then(|section| section.split("\n    - name: Install Rust").next())
        .expect("writable Rust homes step must run before toolchain installation");
    for contract in [
        "[ ! -w \"$rustup_home\" ]",
        "rustup_home=\"$RUNNER_TEMP/rustup\"",
        "[ ! -w \"$cargo_home\" ]",
        "cargo_home=\"$RUNNER_TEMP/cargo\"",
        "echo \"RUSTUP_HOME=$rustup_home\"",
        "echo \"CARGO_HOME=$cargo_home\"",
        "echo \"$cargo_home/bin\" >> \"$GITHUB_PATH\"",
    ] {
        assert!(
            fallback.contains(contract),
            "writable Rust home fallback must retain `{contract}`"
        );
    }
}

#[test]
fn rustfmt_lane_selects_writable_rust_homes_before_toolchain_install() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read CI workflow");
    let fmt = workflow
        .split("  fmt:\n")
        .nth(1)
        .and_then(|section| section.split("\n  deny:\n").next())
        .expect("Format job must remain present");

    let homes = fmt
        .split("- name: Select writable Rust homes for rustfmt")
        .nth(1)
        .and_then(|section| {
            section
                .split("\n      - name: Install Rust toolchain with rustfmt")
                .next()
        })
        .expect("rustfmt lane must select writable homes before rustup runs");
    for contract in [
        "rustup_home=\"$RUNNER_TEMP/rustup\"",
        "cargo_home=\"$RUNNER_TEMP/cargo\"",
        "echo \"RUSTUP_HOME=$rustup_home\"",
        "echo \"CARGO_HOME=$cargo_home\"",
        "echo \"$cargo_home/bin\" >> \"$GITHUB_PATH\"",
    ] {
        assert!(
            homes.contains(contract),
            "rustfmt writable-home guard must retain `{contract}`"
        );
    }
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
fn linux_build_preflight_installs_nothing_when_prerequisites_are_present() {
    // The preflight must be a no-op on an already-provisioned runner. It runs
    // on every Rust job, so a probe that can never be satisfied would drag an
    // `apt-get update` onto all of them.
    //
    // This replaces a test that pinned `libxdo-dev` being probed through
    // `dpkg-query` rather than pkg-config (libxdo ships no `xdo.pc`, so
    // `pkg-config --exists xdo` always fails and forced a reinstall every
    // time). That probe is gone because the package is gone: desktop
    // dependencies now belong to the one job that builds a GUI. The property
    // worth keeping is the general one — present prerequisites mean no apt.
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
    // Every `pkg-config --exists` the preflight asks about is satisfied.
    write_executable(&fake_bin.join("pkg-config"), "#!/bin/sh\nexit 0\n");
    write_executable(&fake_bin.join("id"), "#!/bin/sh\nprintf '0\\n'\n");
    write_executable(
        &fake_bin.join("apt-get"),
        "#!/bin/sh\n: > \"$APT_MARKER\"\nexit 0\n",
    );

    let apt_marker = temp.path().join("apt-ran");
    let status = Command::new("bash")
        .arg("-c")
        .arg(preflight)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("APT_MARKER", &apt_marker)
        .status()
        .expect("run Linux prerequisite preflight");

    assert!(status.success(), "prerequisite preflight must succeed");
    assert!(
        !apt_marker.exists(),
        "a fully provisioned runner must not trigger apt-get"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shared_rust_setup_does_not_require_desktop_packages() {
    // `setup-rust-kache` runs on every Rust job, so anything it installs has to
    // exist on every image behind the `ci-pool-rust` label. That pool mixes
    // runner images, and `libwebkit2gtk-4.1-dev` is absent on Ubuntu focal —
    // when a job landed there, apt exited 100 and the job died in setup before
    // compiling anything. Desktop dependencies belong to the single job that
    // builds a GUI, which installs them itself and legitimately requires
    // webkit2gtk 4.1 for Tauri v2.
    let action =
        fs::read_to_string(repo_root().join(".github/actions/setup-rust-kache/action.yml"))
            .expect("read setup-rust-kache action");
    let action: serde_yaml::Value = serde_yaml::from_str(&action).expect("parse composite action");
    let preflight = action["runs"]["steps"]
        .as_sequence()
        .and_then(|steps| steps.first())
        .and_then(|step| step["run"].as_str())
        .expect("first action step has a shell preflight");

    let packages = preflight
        .lines()
        .find(|line| line.trim_start().starts_with("packages="))
        .expect("preflight declares a packages list");

    for desktop in [
        "libwebkit2gtk",
        "libgtk-3-dev",
        "libayatana-appindicator3-dev",
        "librsvg2-dev",
        "libxdo-dev",
    ] {
        assert!(
            !packages.contains(desktop),
            "shared Rust setup must stay portable across runner images, but its \
             package list contains the desktop dependency `{desktop}`; install it \
             in the job that needs a GUI instead"
        );
    }
}

#[test]
fn docs_only_changes_skip_expensive_runtime_categories() {
    let out = classify("pull_request", &["docs/runtime/CICD.md", "docs/README.md"]);
    assert_eq!(out["docs"], "true");
    assert_eq!(out["rust_compile"], "false");
    assert_eq!(out["rust_test"], "false");
    assert_eq!(out["web"], "false");
    assert_eq!(out["npm"], "false");
    assert_eq!(out["docker"], "false");
    assert_eq!(out["security"], "false");
    assert_eq!(out["release"], "false");
    // Canonical prose participates in docs-check because the recipe also
    // validates repository-local Markdown links.
    assert_eq!(out["docs_check"], "true");
}

#[test]
fn historical_doc_work_products_skip_docs_check() {
    for path in [
        "docs/archive/retired-labby/README.md",
        "docs/sessions/2026-08-18-example.md",
        "docs/superpowers/plans/example.md",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["docs"], "true", "{path}");
        assert_eq!(out["docs_check"], "false", "{path}");
        assert_eq!(out["rust_compile"], "false", "{path}");
        assert_eq!(out["rust_test"], "false", "{path}");
    }
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
fn live_e2e_orchestrator_binds_release_binary_and_verifiable_evidence() {
    let script = fs::read_to_string(repo_root().join("scripts/ci/labby-live-e2e.sh"))
        .expect("read live E2E orchestrator");
    assert!(script.contains("export LABBY_E2E_BINARY="));
    assert!(script.contains("LABBY_RELEASE_BINARY"));
    assert!(script.contains("live-identity-protected-restart"));
    assert!(script.contains("live-http-observability"));
    assert!(script.contains("live-http-ipv6"));
    assert!(script.contains("residual-audit.json"));
    assert!(!script.contains("\"signature\""));
    assert!(script.contains("child_root=\"$run_root/repeats/seed-$repeat_seed\""));
    assert!(script.contains("LABBY_E2E_RUN_ROOT=\"$child_root\""));
    assert!(script.contains("repeat10.json"));
    assert!(script.contains("group_has_listener"));
    assert!(script.contains("trap 'cancel 143' TERM"));
    assert!(script.contains(
        "LABBY_LIVE_BROWSER_NIGHTLY=\"$([ \"$tier\" = nightly ] && echo true || echo false)\""
    ));
    let coverage = script
        .rfind("coverage.json.sha256")
        .expect("final coverage checksum");
    let retained_scan = script
        .rfind("grep -R -a -F -f")
        .expect("final retained scan");
    assert!(
        retained_scan < coverage,
        "retained scan must pass before coverage and checksum claim success"
    );
}

#[test]
fn live_e2e_ci_routes_scheduled_and_manual_events_to_extended_tiers() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read CI workflow");
    assert!(workflow.contains("github.event_name == 'schedule' && 'nightly'"));
    assert!(workflow.contains("github.event_name == 'workflow_dispatch' && 'manual'"));
    assert!(workflow.contains("labby-live-e2e.sh \"$LABBY_E2E_TIER\""));
}

#[test]
fn explicit_policy_files_route_to_the_right_checks() {
    let labeler = classify("pull_request", &[".github/labeler.yml"]);
    assert_eq!(labeler["workflow"], "true");

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
fn auth_matrix_changes_route_to_conformance() {
    for path in [
        "conformance/auth-requirements.json",
        "conformance/mcp-auth-coverage-manifest.json",
        "scripts/ci/test_auth_spec_matrix.py",
        "conformance/mcp-auth-normative.json",
        "conformance/openai-auth-normative.json",
        "scripts/ci/refresh_mcp_auth_denominator.py",
        "scripts/ci/refresh_openai_auth_denominator.py",
        "scripts/ci/publish_mcp_auth_disposition.py",
        "scripts/ci/openai-auth-conformance.sh",
        "scripts/ci/auth_backup_restore_drill.py",
    ] {
        let out = classify("pull_request", &[path]);
        assert_eq!(out["workflow"], "true", "{path}");
        assert_eq!(out["rust_test"], "true", "{path}");
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
fn protected_docs_workflow_is_trusted_and_label_gated() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/protected-docs.yml"))
        .expect("read protected docs workflow")
        .replace("\r\n", "\n");

    assert!(workflow.contains("pull_request_target:"));
    assert!(workflow.contains("name: Protected docs guard"));
    assert!(workflow.contains("ref: ${{ github.event.pull_request.base.sha }}"));
    assert!(workflow.contains("persist-credentials: false"));
    assert!(workflow.contains("protected-docs-approved"));
    assert!(workflow.contains("scripts/ci/protected_doc_guard.py"));
    assert!(!workflow.contains("github.event.pull_request.head.sha"));
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
        workflow.contains("check_workflow_policy.py") && workflow.contains("runs-on: ubuntu-24.04"),
        "CI must enforce the hosted-runner policy on a GitHub-hosted runner"
    );
    assert!(
        workflow.contains("success|skipped"),
        "ci-gate must accept intentionally skipped jobs"
    );
    for required in [
        "gateway-admin-browser",
        "live-e2e-core",
        "codemode-runner-smoke",
        "mcp-regressions",
        "palette-web",
        "palette-rust",
        "palette-windows",
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
    for advisory in ADVISORY_JOBS {
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
        gate.contains("HEAD_REPOSITORY") && gate.contains("fork safety"),
        "ci-gate must document the narrow fork-safety exception for skipped changes"
    );
    let unconditional = "fleet-policy";
    assert!(
        gate.contains(&format!("require_success {unconditional} ")),
        "ci-gate must reject a skipped `{unconditional}` job"
    );
    assert!(
        gate.contains("require_success changes "),
        "ci-gate must still require changes on trusted branches"
    );
    assert!(
        gate.contains("needs.changes.outputs.gate_key_drift"),
        "ci-gate must surface routing keys the trusted classifier could not emit"
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
        "Playwright must use a stable browser path on the hosted runner"
    );
    for library in ["libasound2t64", "libgbm1", "libnss3", "libxkbcommon0"] {
        assert!(
            browser_job.contains(library),
            "Hosted Ubuntu runners must install the Chromium runtime library {library}"
        );
    }
    assert!(
        !browser_job.contains("playwright install-deps"),
        "Hosted Ubuntu runners must install explicit runtime libraries"
    );
    assert!(browser_job.contains("Install Playwright browser"));
    assert!(browser_job.contains("pnpm exec playwright install chromium"));
    assert!(browser_job.contains("Verify Playwright browser launch"));
    assert!(browser_job.contains("chromium.executablePath()"));
    assert!(browser_job.contains("fs.existsSync(executable)"));
    assert!(browser_job.contains("chromium.launch({ headless: true })"));
    assert!(
        browser_job.contains("pnpm exec playwright install chromium"),
        "Hosted Ubuntu runners must install the Playwright browser"
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
        feature_slices.contains("slice: [gateway, gateway-host, integrated-gateway, fs, skills]"),
        "CI must compile every distributable gateway profile and the standalone Skills slice"
    );
    assert!(
        feature_slices.contains("if: matrix.slice == 'skills'")
            && feature_slices.contains("--no-default-features --features skills"),
        "the standalone Skills slice must retain its focused runtime regression"
    );
    assert!(
        feature_slices.contains(
            "cargo test -p labby --no-default-features --features fs --locked --test doctor_proxy_preflight"
        ),
        "the fs slice must run the proxy preflight integration binary without gateway"
    );
    assert!(
        feature_slices.contains("--features ${{ matrix.slice }} --lib --bins --locked"),
        "feature slices must warm the product/native dependency graph at normal concurrency"
    );
    assert!(
        feature_slices.contains("--features ${{ matrix.slice }} --all-targets --locked"),
        "feature slices must retain the all-target completion pass after warm-up"
    );
    assert!(
        !feature_slices.contains("-j 1"),
        "feature slices must not change Cargo job count because that changes native build-script NUM_JOBS"
    );

    let clippy_job = workflow
        .split("  clippy:\n")
        .nth(1)
        .and_then(|section| section.split("\n  mcp-regressions:").next())
        .expect("clippy job");
    assert!(
        clippy_job.contains("cargo clippy --workspace --exclude labby --all-features"),
        "Clippy must lint extracted workspace targets separately from the monolithic product crate"
    );
    let clippy_warm = clippy_job
        .find("cargo clippy -p labby --all-features --lib --bins --locked")
        .expect("Clippy warm-up command");
    let extracted_lint = clippy_job
        .find("cargo clippy --workspace --exclude labby --all-features")
        .expect("extracted workspace Clippy command");
    let clippy_test_graph_warm = clippy_job
        .find("--test architecture_boundaries --locked -- -D warnings")
        .expect("Clippy test dependency graph warm-up command");
    let clippy_all_targets = clippy_job
        .find("cargo clippy -p labby --all-features --all-targets --locked")
        .expect("all-target Labby Clippy command");
    assert!(
        clippy_warm < extracted_lint,
        "Clippy must warm Labby and Labby Gateway normal targets before any extracted all-target lint fan-out"
    );
    assert!(
        extracted_lint < clippy_test_graph_warm && clippy_test_graph_warm < clippy_all_targets,
        "Clippy must warm the dev-dependency feature graph in isolation before its all-target pass"
    );
    assert!(
        !clippy_job.contains("-j 1"),
        "Clippy must not change Cargo job count because that changes native build-script NUM_JOBS"
    );

    let msrv_job = workflow
        .split("  msrv:\n")
        .nth(1)
        .and_then(|section| section.split("\n  codemode-runner-smoke:").next())
        .expect("msrv job");
    let gateway_msrv_warm = msrv_job
        .find("cargo +1.97.1 check -p labby-gateway --all-features")
        .expect("gateway MSRV test-target warm-up command");
    let workspace_msrv = msrv_job
        .find("cargo +1.97.1 check --workspace --all-features --all-targets --locked")
        .expect("required workspace MSRV command");
    assert!(
        msrv_job.contains("--all-targets --locked") && gateway_msrv_warm < workspace_msrv,
        "MSRV must warm the gateway test target before preserving the exact required workspace command"
    );

    let mcp_regressions = workflow
        .split("  mcp-regressions:\n")
        .nth(1)
        .and_then(|section| section.split("\n  mcp-conformance:").next())
        .expect("mcp-regressions job");
    assert!(
        mcp_regressions.contains("cargo build -p labby --all-features --lib --bins --locked"),
        "MCP regressions must warm normal Labby/Gateway targets before test harness compilation"
    );

    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(
        release.matches("skills --help").count() >= 3
            && release
                .matches("Read Agent Skills visible to the local CLI")
                .count()
                >= 3,
        "Unix, Windows, and container release artifacts must prove the compiled Skills surface"
    );
    let incus_smoke = fs::read_to_string(repo_root().join("scripts/ci/smoke-incus-image.sh"))
        .expect("read Incus smoke script");
    assert!(
        incus_smoke.contains("labby skills --help")
            && incus_smoke.contains("Read Agent Skills visible to the local CLI"),
        "the baked Incus binary must prove the compiled Skills surface"
    );

    let test_job = workflow
        .split("  test:\n")
        .nth(1)
        .and_then(|section| section.split("\n  test-fork:").next())
        .expect("test job");
    let test_fork_job = workflow
        .split("  test-fork:\n")
        .nth(1)
        .and_then(|section| section.split("\n  test-windows:").next())
        .expect("test-fork job");
    for (name, job) in [("test", test_job), ("test-fork", test_fork_job)] {
        assert!(
            job.contains("cargo build -p labby --all-features --lib --bins --locked"),
            "{name} must warm normal Labby/Gateway targets before nextest fan-out"
        );
    }

    // Read the job env structurally: the rationale comments below mention
    // CARGO_BUILD_JOBS by name, so a substring check would match the very
    // explanation of why it is absent.
    let parsed = ci_workflow_yaml(&workflow);
    for job in [
        "feature-slices",
        "mcp-regressions",
        "test",
        "test-fork",
        "rust-coverage",
    ] {
        let env = &parsed["jobs"][job]["env"];
        // These jobs must NOT pin CARGO_BUILD_JOBS. Cargo forwards it to every
        // build script as NUM_JOBS, and aws-lc-sys compiles 414 C and 902
        // assembly sources through the cc crate — Kache cannot cache them,
        // because it wraps rustc, not cc. Pinning it to 1 serialized ~1300
        // uncached sources, turning ~8-minute builds into 30+ and letting the
        // autoscaling runners reclaim them mid-link. Keep that job-wide knob
        // unset even when Rust target concurrency needs shaping. Recent product
        // growth can put the normal library and lib-test harness over the
        // runner memory ceiling when rustc launches them cold together. The
        // safe pattern is to phase-separate a normal-concurrency warm-up and
        // the all-target pass without ever changing Cargo's job count.
        assert!(
            env["CARGO_BUILD_JOBS"].is_null(),
            "{job} must not throttle Cargo build jobs; that also serializes the aws-lc-sys C build, which no cache can absorb"
        );
        assert_eq!(
            env["RUSTFLAGS"].as_str(),
            Some("-C linker=clang -C link-arg=-fuse-ld=lld"),
            "{job} must use the lower-memory lld linker"
        );
    }
    assert!(
        parsed["jobs"]["clippy"]["env"]["CARGO_BUILD_JOBS"].is_null(),
        "Clippy must phase-separate product targets, never set job-wide CARGO_BUILD_JOBS"
    );
}

/// Routing keys that `ci.yml` gates on but the classifier never emits, because
/// the `changes` job synthesizes them at runtime.
const RUNTIME_ONLY_CHANGE_OUTPUTS: &[&str] = &["gate_key_drift"];

/// Jobs that stay visible on pull requests but must not block `ci-gate`.
const ADVISORY_JOBS: &[&str] = &["test-windows"];

fn gated_changed_path_keys(workflow: &str) -> BTreeSet<String> {
    workflow
        .split("needs.changes.outputs.")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|key| !key.is_empty())
        .collect()
}

fn ci_workflow_text() -> String {
    fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml")
}

fn ci_workflow_yaml(text: &str) -> serde_yaml::Value {
    serde_yaml::from_str(text).expect("parse ci.yml")
}

#[test]
fn fork_pull_requests_use_hosted_ci() {
    let workflow = ci_workflow_yaml(include_str!("../../../.github/workflows/ci.yml"));
    let changes = &workflow["jobs"]["changes"];
    assert_eq!(changes["runs-on"].as_str(), Some("ubuntu-24.04"));
    assert_eq!(
        changes["if"].as_str(),
        Some(
            "${{ github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository }}"
        )
    );
}

/// Adding a routing key to `changed_paths.py` and gating a job on it are only
/// safe together when the key is also declared as a `changes` job output, is
/// forwarded from the identically-named classifier output, and is emitted by
/// the classifier. Break any link and the gate reads as the empty string, so
/// the job skips and `ci-gate` accepts the skip.
#[test]
fn gated_changed_path_keys_are_declared_and_classifier_backed() {
    let workflow_text = ci_workflow_text();
    let workflow = ci_workflow_yaml(&workflow_text);
    let outputs = workflow["jobs"]["changes"]["outputs"]
        .as_mapping()
        .expect("changes job declares outputs");
    let declared: BTreeSet<String> = outputs
        .keys()
        .map(|key| key.as_str().expect("output name").to_string())
        .collect();
    let emitted: BTreeSet<String> = classify("pull_request", &["README.md"])
        .into_keys()
        .collect();

    // The reconciler in the classify step and this test both discover gates by
    // scanning for `needs.changes.outputs.<key>`. GitHub also accepts
    // `needs.changes.outputs['key']`, which neither would see — keep the one
    // form so a gate can never hide from both.
    assert!(
        !workflow_text.contains("needs.changes.outputs["),
        "use `needs.changes.outputs.<key>`; the bracket form is invisible to the classify step's reconciler"
    );

    for key in gated_changed_path_keys(&workflow_text) {
        assert!(
            declared.contains(&key),
            "ci.yml gates on `needs.changes.outputs.{key}` but the changes job does not declare that output; the gate would read as an empty string and skip the job"
        );
        if RUNTIME_ONLY_CHANGE_OUTPUTS.contains(&key.as_str()) {
            continue;
        }
        assert!(
            emitted.contains(&key),
            "ci.yml gates on `{key}` but scripts/ci/changed_paths.py never emits it"
        );
    }

    for (name, expression) in outputs {
        let name = name.as_str().expect("output name");
        let expression = expression.as_str().expect("output expression").trim();
        assert_eq!(
            expression,
            format!("${{{{ steps.classify.outputs.{name} }}}}"),
            "the `{name}` job output must forward the identically-named classify output; a mismatch resolves to the empty string and silently skips every job gated on it"
        );
        if RUNTIME_ONLY_CHANGE_OUTPUTS.contains(&name) {
            continue;
        }
        assert!(
            emitted.contains(name),
            "the changes job exports `{name}` but scripts/ci/changed_paths.py never emits it"
        );
    }
}

/// `ci-gate` runs with `if: always()`, so a job missing from its `needs:` list —
/// or present there but missing its `require_*` assertion — cannot fail the
/// build. That is the same vacuously-green shape as a silently skipped gate.
#[test]
fn ci_gate_aggregates_every_non_advisory_job() {
    let workflow_text = ci_workflow_text();
    let workflow = ci_workflow_yaml(&workflow_text);
    let jobs: BTreeSet<String> = workflow["jobs"]
        .as_mapping()
        .expect("ci.yml declares jobs")
        .keys()
        .map(|name| name.as_str().expect("job name").to_string())
        .collect();
    let gate = &workflow["jobs"]["ci-gate"];
    let aggregated: BTreeSet<String> = gate["needs"]
        .as_sequence()
        .expect("ci-gate declares needs")
        .iter()
        .map(|need| need.as_str().expect("job name").to_string())
        .collect();
    let checks = gate["steps"]
        .as_sequence()
        .expect("ci-gate steps")
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    for name in &jobs {
        if name == "ci-gate" || ADVISORY_JOBS.contains(&name.as_str()) {
            continue;
        }
        assert!(
            aggregated.contains(name),
            "ci-gate does not aggregate `{name}`; a failure there would not block the build"
        );
        assert!(
            checks.contains(&format!("needs.{name}.result")),
            "ci-gate lists `{name}` in needs but never asserts its result"
        );
    }

    for name in &aggregated {
        assert!(
            jobs.contains(name),
            "ci-gate needs `{name}`, which is not a job in this workflow"
        );
        assert!(
            !ADVISORY_JOBS.contains(&name.as_str()),
            "advisory job `{name}` must not block ci-gate"
        );
    }
}

/// A stand-in for a base commit whose classifier predates the `unraid` key.
#[cfg(unix)]
const STALE_CLASSIFIER: &str = r#"import argparse
from pathlib import Path

keys = "all docs docs_check workflow rust_compile rust_test web palette npm docker security release".split()
parser = argparse.ArgumentParser()
parser.add_argument("--event", required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--write-changed-files", type=Path)
args, _ = parser.parse_known_args()
if args.write_changed_files:
    args.write_changed_files.write_text("unraid/labby.plg\n")
args.output.write_text("".join(f"{key}=false\n" for key in keys))
"#;

#[cfg(unix)]
struct ClassifyRun {
    succeeded: bool,
    outputs: String,
    log: String,
}

#[cfg(unix)]
fn classify_step_script() -> String {
    let workflow = ci_workflow_yaml(&ci_workflow_text());
    workflow["jobs"]["changes"]["steps"]
        .as_sequence()
        .expect("changes job steps")
        .iter()
        .find(|step| step["id"].as_str() == Some("classify"))
        .and_then(|step| step["run"].as_str())
        .expect("classify step runs a shell script")
        .to_string()
}

/// A working directory shaped like the runner's: a checkout whose `ci.yml` the
/// classify step reads back to discover which routing keys jobs gate on.
#[cfg(unix)]
fn classify_sandbox() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("create classify sandbox");
    fs::create_dir_all(temp.path().join(".github/workflows")).expect("create workflow directory");
    fs::copy(
        repo_root().join(".github/workflows/ci.yml"),
        temp.path().join(".github/workflows/ci.yml"),
    )
    .expect("copy ci.yml into the sandbox");
    temp
}

#[cfg(unix)]
fn run_classify_step(root: &Path, script: &str, classifier: &Path) -> ClassifyRun {
    let github_output = root.join("github_output.txt");
    let step_summary = root.join("step_summary.md");
    let result = Command::new("bash")
        .arg("-c")
        .arg(script)
        .current_dir(root)
        .env("LABBY_CHANGED_PATHS", classifier)
        .env("EVENT_NAME", "pull_request")
        .env("GITHUB_OUTPUT", &github_output)
        .env("GITHUB_STEP_SUMMARY", &step_summary)
        .output()
        .expect("run the classify step");
    ClassifyRun {
        succeeded: result.status.success(),
        outputs: fs::read_to_string(&github_output).unwrap_or_default(),
        log: format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ),
    }
}

/// The `changes` job deliberately runs the base commit's classifier so a pull
/// request cannot reroute its own CI. `ci.yml` itself comes from the merge ref,
/// so a pull request that adds a routing key gates on a key the trusted
/// classifier cannot emit. That must fail open to running the gated job — the
/// old behavior skipped it silently and still satisfied `ci-gate`.
#[cfg(unix)]
#[test]
fn classify_step_fails_open_when_the_trusted_classifier_omits_a_gated_key() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    let classifier = root.join("stale_classifier.py");
    fs::write(&classifier, STALE_CLASSIFIER).expect("write stale classifier");

    let run = run_classify_step(root, &classify_step_script(), &classifier);
    assert!(run.succeeded, "classify step failed:\n{}", run.log);

    let outputs = &run.outputs;
    assert!(
        outputs.lines().any(|line| line == "unraid=true"),
        "a gated key the trusted classifier omits must default to true so the job runs, got:\n{outputs}"
    );
    assert!(
        outputs
            .lines()
            .any(|line| line.starts_with("gate_key_drift=") && line.contains("unraid")),
        "the reconciled keys must be reported to ci-gate, got:\n{outputs}"
    );
    assert!(
        outputs.lines().any(|line| line == "rust_test=false"),
        "reconciliation must never rewrite a key the trusted classifier did emit, got:\n{outputs}"
    );
    // The annotation is the only operator-facing signal on the fail-open path.
    assert!(
        run.log.contains("Changed-path routing drift") && run.log.contains("'unraid'"),
        "fail-open must annotate the run with the reconciled key, got:\n{}",
        run.log
    );
}

/// A malformed value fails `== 'true'` exactly like a missing one, so presence
/// alone is not enough to conclude the gate will work.
#[cfg(unix)]
#[test]
fn classify_step_reconciles_a_malformed_value_like_a_missing_key() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    let classifier = root.join("malformed_classifier.py");
    fs::write(
        &classifier,
        STALE_CLASSIFIER.replace(
            r#"args.output.write_text("".join(f"{key}=false\n" for key in keys))"#,
            r#"args.output.write_text("".join(f"{key}=false\n" for key in keys) + "unraid=True")"#,
        ),
    )
    .expect("write malformed classifier");

    let run = run_classify_step(root, &classify_step_script(), &classifier);
    assert!(run.succeeded, "classify step failed:\n{}", run.log);
    assert!(
        run.outputs.lines().any(|line| line == "unraid=true"),
        "`unraid=True` does not satisfy `== 'true'`, so it must be reconciled, got:\n{}",
        run.outputs
    );
    assert!(
        !run.outputs.lines().any(|line| line == "unraid=True"),
        "the malformed value must be replaced, not shadowed, got:\n{}",
        run.outputs
    );
}

/// Writes a stand-in for the branch's own classifier at the path the classify
/// step re-runs for the base/branch union.
#[cfg(unix)]
fn write_branch_classifier(root: &Path, values: &str) {
    fs::create_dir_all(root.join("scripts/ci")).expect("create scripts/ci");
    fs::write(
        root.join("scripts/ci/changed_paths.py"),
        format!(
            r#"import argparse
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--event", required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--changed-files", type=Path)
parser.add_argument("--write-changed-files", type=Path)
args, _ = parser.parse_known_args()
args.output.write_text({values:?})
"#
        ),
    )
    .expect("write branch classifier");
}

/// Pinning the classifier to the base commit also pins its path -> category
/// mappings, so a branch that routes a new directory into an existing category
/// gets a well-formed `false` and the gated job skips for real. The branch's
/// own classifier is unioned in to fix that — but only in the broadening
/// direction, or a branch could switch its own checks off.
#[cfg(unix)]
#[test]
fn classify_step_unions_the_branch_classifier_but_never_lets_it_narrow() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    let trusted = root.join("trusted_classifier.py");
    fs::write(
        &trusted,
        STALE_CLASSIFIER
            .replace(
                r#"args.output.write_text("".join(f"{key}=false\n" for key in keys))"#,
                r#"args.output.write_text("".join(f"{key}=false\n" for key in keys) + "unraid=false\nrust_test=true\n")"#,
            )
            .replace("unraid/labby.plg", "apps/palette-v2/App.tsx"),
    )
    .expect("write trusted classifier");
    // The branch knows a mapping the base commit does not, and also tries to
    // switch the workspace test suite off.
    write_branch_classifier(root, "palette=true\nrust_test=false\nweb=false\n");

    let run = run_classify_step(root, &classify_step_script(), &trusted);
    assert!(run.succeeded, "classify step failed:\n{}", run.log);

    let outputs = &run.outputs;
    assert!(
        outputs.lines().any(|line| line == "palette=true"),
        "a category the branch classifier routes to must run even when the base commit's mapping predates it, got:\n{outputs}"
    );
    assert!(
        outputs.lines().any(|line| line == "rust_test=true"),
        "the branch classifier must never lower a trusted `true`, got:\n{outputs}"
    );
    assert!(
        outputs.lines().any(|line| line == "web=false"),
        "keys both classifiers call false must stay false, got:\n{outputs}"
    );
    assert!(
        run.log.contains("routing broadened") && run.log.contains("palette"),
        "broadening must be annotated on the run, got:\n{}",
        run.log
    );
}

/// The union is an enhancement, not a dependency: a branch classifier that
/// cannot run must degrade to trusted-only routing, not fail the build.
#[cfg(unix)]
#[test]
fn classify_step_survives_a_broken_branch_classifier() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    let trusted = root.join("trusted_classifier.py");
    fs::write(&trusted, STALE_CLASSIFIER).expect("write trusted classifier");
    fs::create_dir_all(root.join("scripts/ci")).expect("create scripts/ci");
    fs::write(
        root.join("scripts/ci/changed_paths.py"),
        "import sys\nsys.exit(3)\n",
    )
    .expect("write broken branch classifier");

    let run = run_classify_step(root, &classify_step_script(), &trusted);
    assert!(
        run.succeeded,
        "a broken branch classifier must not fail routing:\n{}",
        run.log
    );
    assert!(
        run.outputs.lines().any(|line| line == "rust_test=false"),
        "trusted routing must survive intact, got:\n{}",
        run.outputs
    );
    assert!(
        run.outputs.lines().any(|line| line == "unraid=true"),
        "reconciliation must still fail open for keys the trusted classifier omits, got:\n{}",
        run.outputs
    );
    assert!(
        run.log.contains("branch's own classifier failed to run"),
        "the degraded path must be annotated, got:\n{}",
        run.log
    );
}

/// The healthy case: with an in-tree classifier every gated key is emitted, so
/// nothing is reconciled and no drift is reported.
#[cfg(unix)]
#[test]
fn classify_step_reports_no_drift_when_the_classifier_emits_every_gated_key() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    fs::write(root.join("pinned-changed-files.txt"), "unraid/labby.plg\n")
        .expect("seed changed files");

    let classifier = root.join("changed_paths.py");
    fs::copy(repo_root().join("scripts/ci/changed_paths.py"), &classifier)
        .expect("copy the in-tree classifier");

    // The classifier resolves its own diff from git when no explicit file list
    // is given; pin the list so the sandbox needs no git history. Use a file
    // the step does not also rewrite through `--write-changed-files`.
    let script = classify_step_script();
    let pinned = script.replace(
        "--event \"$EVENT_NAME\" \\",
        "--event \"$EVENT_NAME\" --changed-files pinned-changed-files.txt \\",
    );
    assert_ne!(
        pinned, script,
        "the classify step no longer invokes the classifier in the shape this test patches; \
         without the patch the classifier sees an empty path list and returns every key true, \
         which would make this test pass while checking nothing"
    );

    let run = run_classify_step(root, &pinned, &classifier);
    assert!(run.succeeded, "classify step failed:\n{}", run.log);

    let outputs = &run.outputs;
    assert!(
        outputs.lines().any(|line| line == "gate_key_drift="),
        "an in-tree classifier must produce no routing drift, got:\n{outputs}"
    );
    assert!(
        outputs.lines().any(|line| line == "unraid=true"),
        "unraid plugin changes must still enable the plugin check, got:\n{outputs}"
    );
    // The negative control: proves a real classification happened rather than
    // the classifier's empty-path-list fallback, which returns every key true.
    assert!(
        outputs.lines().any(|line| line == "web=false"),
        "an unrelated key must stay false, otherwise this test is passing on the all-true fallback, got:\n{outputs}"
    );
}

/// A gate whose key the `changes` job never forwards as a job output always
/// reads as the empty string, whatever the classifier emits. Reconciliation
/// cannot repair that, so it must fail the build rather than warn.
#[cfg(unix)]
#[test]
fn classify_step_fails_when_a_gate_has_no_matching_job_output() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    let workflow = root.join(".github/workflows/ci.yml");
    // Add a gate on a key nothing forwards, leaving the existing gates intact.
    let patched = fs::read_to_string(&workflow)
        .expect("read sandbox ci.yml")
        .replace(
            "if: ${{ needs.changes.outputs.unraid == 'true' }}",
            "if: ${{ needs.changes.outputs.unraid == 'true' && needs.changes.outputs.undeclared_key == 'true' }}",
        );
    assert!(
        patched.contains("needs.changes.outputs.undeclared_key"),
        "the unraid gate no longer has the shape this test patches"
    );
    fs::write(&workflow, patched).expect("write patched ci.yml");

    let classifier = root.join("classifier.py");
    fs::copy(repo_root().join("scripts/ci/changed_paths.py"), &classifier)
        .expect("copy the in-tree classifier");

    let run = run_classify_step(root, &classify_step_script(), &classifier);
    assert!(
        !run.succeeded,
        "a gate with no matching job output must fail the build, got:\n{}",
        run.log
    );
    assert!(
        run.log.contains("undeclared_key"),
        "the failure must name the unforwarded key, got:\n{}",
        run.log
    );
}

/// The reconciler discovers gates by reading `ci.yml` back. If that discovery
/// silently found nothing it would reinstate the exact bug it exists to close,
/// so it must fail loudly instead.
#[cfg(unix)]
#[test]
fn classify_step_fails_when_it_cannot_enumerate_gates() {
    let sandbox = classify_sandbox();
    let root = sandbox.path();
    fs::remove_file(root.join(".github/workflows/ci.yml")).expect("remove sandbox ci.yml");
    let classifier = root.join("stale_classifier.py");
    fs::write(&classifier, STALE_CLASSIFIER).expect("write stale classifier");

    let run = run_classify_step(root, &classify_step_script(), &classifier);
    assert!(
        !run.succeeded,
        "losing track of ci.yml must fail the build rather than report no drift, got:\n{}",
        run.log
    );
    assert!(
        !run.outputs.contains("gate_key_drift="),
        "a failed enumeration must not claim there was no drift, got:\n{}",
        run.outputs
    );
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
    for target in [
        "target: x86_64-unknown-linux-gnu",
        "target: aarch64-apple-darwin",
        "target: x86_64-pc-windows-msvc",
    ] {
        assert!(release.contains(target), "release matrix missing {target}");
    }
    assert!(release.contains("runner: '\"macos-15\"'"));
    assert!(!release.contains("x86_64-apple-darwin"));
    assert!(!release.contains("/latest/download/"));
    assert!(!release.contains("mcp-publisher"));
    assert!(!release.contains("registry.modelcontextprotocol.io"));
    let registry = fs::read_to_string(repo_root().join(".github/workflows/mcp-registry.yml"))
        .expect("read MCP Registry workflow");
    assert!(registry.contains("mcp-registry-publish.yml@b2813662ca27ca8868752fb353d9dd568f2f97f9"));
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

/// Regression guard for lab-k222n.
///
/// The release container compiles the real `labby` crate inside Docker. Several
/// runtime resources are embedded with `include_str!`, so excluding their source
/// trees from the Docker context makes the release-only build fail even though
/// native CI is green.
#[test]
fn release_container_includes_compile_time_contract_and_skill_assets() {
    let dockerignore =
        fs::read_to_string(repo_root().join(".dockerignore")).expect("read Docker ignore rules");
    let dockerfile =
        fs::read_to_string(repo_root().join("config/Dockerfile")).expect("read release Dockerfile");

    for required in [
        "!docs/contracts/**",
        "!plugins/labby/skills/using-labby/**",
        "!plugins/labby/skills/creating-snippets/**",
    ] {
        assert!(
            dockerignore.contains(required),
            "Docker context must retain embedded asset rule {required}"
        );
    }

    for required in [
        "COPY docs/contracts/ docs/contracts/",
        "COPY plugins/labby/skills/using-labby/ plugins/labby/skills/using-labby/",
        "COPY plugins/labby/skills/creating-snippets/ plugins/labby/skills/creating-snippets/",
    ] {
        assert!(
            dockerfile.contains(required),
            "release Dockerfile must copy embedded assets with {required}"
        );
    }
}

/// Regression guard for lab-bm6pc.
///
/// Incus marks a system container RUNNING before systemd's system bus is ready.
/// The bootstrap uses `hostnamectl` and `systemctl`, so it must explicitly wait
/// for the guest service manager after launch/start and before those calls.
#[test]
fn incus_bootstrap_waits_for_guest_systemd_before_systemctl_consumers() {
    let script = fs::read_to_string(repo_root().join("scripts/incus-bootstrap.sh"))
        .expect("read Incus bootstrap script");
    assert!(
        script.contains("\nwait_for_guest_systemd\nverify_container_substrate\n"),
        "guest systemd readiness must precede bootstrap operations that consume the system bus"
    );
    assert!(
        script.contains("systemctl is-system-running"),
        "readiness must probe the guest system manager rather than only Incus RUNNING state"
    );
}
