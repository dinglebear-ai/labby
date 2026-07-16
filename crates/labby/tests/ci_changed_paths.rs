use std::collections::HashMap;
use std::fs;
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
    assert_eq!(out["docs_check"], "true");
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

    let gitleaks = classify("pull_request", &[".gitleaksignore"]);
    assert_eq!(gitleaks["security"], "true");
    assert_eq!(gitleaks["rust_compile"], "false");
    assert_eq!(gitleaks["rust_test"], "false");

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
fn workflow_changes_enable_everything() {
    let out = classify("pull_request", &[".github/workflows/ci.yml"]);
    for (key, value) in out {
        assert_eq!(value, "true", "{key} should be true for workflow changes");
    }
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
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml");

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
        "palette-windows",
        "rust-coverage",
    ] {
        assert!(
            workflow.contains(&format!("- {required}"))
                && workflow.contains(&format!("needs.{required}.result")),
            "ci-gate must aggregate {required}"
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
    assert!(browser_job.contains("pnpm exec playwright install --with-deps chromium"));
    assert!(browser_job.contains("needs.changes.outputs.web == 'true'"));
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
fn release_tool_downloads_are_version_and_digest_pinned() {
    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(!release.contains("/latest/download/"));
    assert!(release.contains("version=\"v1.8.0\""));
    assert!(release.contains("1370446bbe74d562608e8005a6ccce02d146a661fbd78674e11cc70b9618d6cf"));
    assert!(release.contains("sha256sum --check --strict"));
    assert!(release.contains("cosign verify-blob"));
    assert!(release.contains("${archive}.sigstore.json"));
    assert!(release.contains("distrobuilder --classic --revision 2114"));

    let config = fs::read_to_string(repo_root().join("release-please-config.json"))
        .expect("read release-please config");
    assert!(!config.contains("\"skip-github-release\": true"));
    assert!(config.contains("\"draft\": true"));
    assert!(config.contains("\"force-tag-creation\": true"));

    assert!(release.contains("--json isDraft --jq .isDraft"));
    assert!(release.contains("gh release upload \"$RELEASE_TAG\" \"${files[@]}\" --clobber"));
    assert!(release.contains("if [[ \"$DRAFT_CREATED\" == \"true\" ]]"));
    assert!(release.contains("if [[ -f /tmp/labby-new-version-image ]]"));
}

#[test]
fn rolling_incus_release_promotes_verified_immutable_assets_before_tag() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/build-incus-image.yml"))
        .expect("read Incus image workflow");
    let upload = workflow
        .find("gh release upload \"$ROLLING_TAG\" \"$verify_dir\"/* --clobber")
        .expect("rolling release must receive immutable release assets");
    let rolling_verify = workflow
        .find("cd \"$rolling_verify_dir\" && sha256sum --check --strict")
        .expect("rolling assets must be downloaded and checksum-verified");
    let advance = workflow
        .find("git push -f")
        .expect("rolling tag must advance explicitly");
    assert!(
        upload < rolling_verify && rolling_verify < advance,
        "rolling assets must be uploaded and remotely verified before the tag advances"
    );
}

#[test]
fn secret_scan_uses_full_history_and_only_exact_fingerprint_baselines() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read CI workflow");
    let secret_job = workflow
        .split("  secret-scan:")
        .nth(1)
        .and_then(|section| section.split("\n  unraid-plugin-check:").next())
        .expect("secret scan job");
    assert!(
        secret_job.contains("fetch-depth: 0"),
        "secret scan must include history and HEAD"
    );

    let baseline = fs::read_to_string(repo_root().join(".gitleaksignore"))
        .expect("read Gitleaks fingerprint baseline");
    assert!(baseline.contains(
        "17bc2ac442e2350efc4462f10811f089898b22c2:docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md:generic-api-key:100"
    ));
    for (index, line) in baseline.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        assert!(
            line.split(':').count() >= 4 && !line.contains('*'),
            ".gitleaksignore:{} must be an exact commit:path:rule:line fingerprint, not a broad path baseline",
            index + 1
        );
    }

    let policy =
        fs::read_to_string(repo_root().join("docs/runtime/CICD.md")).expect("read CI/CD policy");
    for required in [
        "`commit:path:rule-id:line`",
        "confirmed revoked or retired",
        "current tree is redacted",
        "must never baseline a finding introduced at `HEAD`",
    ] {
        assert!(
            policy.contains(required),
            "CI/CD policy must document revoked-history baseline rule: {required}"
        );
    }
}
