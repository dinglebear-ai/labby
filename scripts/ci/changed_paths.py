#!/usr/bin/env python3
"""Classify changed files into Lab CI routing categories."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from collections.abc import Callable
from pathlib import Path


OUTPUT_KEYS = [
    "all",
    "docs",
    "docs_check",
    "native_plugins",
    "workflow",
    "rust_compile",
    "rust_test",
    "web",
    "browser_extension",
    "desktop",
    "npm",
    "tailcat",
    "incus",
    "security",
    "javascript_advisories",
    "release",
    "unraid",
    "verification",
    "rust_api",
    "rust_features",
    "codemode",
    "mcp_conformance",
    "lifecycle_conformance",
    "installer",
    "extended",
]

# Events that run the extended suites: duplicated or slow lanes that pull
# requests skip (gateway-only runtime shards, focused MCP regressions, MSRV,
# feature slices, macOS installers, and the path-routed conformance lanes).
# Pushes to main and the nightly schedule keep them on the merge path, and
# a manual dispatch on a branch opts a pull request into the full set.
EXTENDED_EVENTS = {"schedule", "workflow_dispatch", "push"}

TOOLCHAIN_INPUTS = {"Cargo.lock", "rust-toolchain.toml"}


def starts(path: str, *prefixes: str) -> bool:
    return any(path == prefix.rstrip("/") or path.startswith(prefix) for prefix in prefixes)


def any_match(paths: list[str], predicate: Callable[[str], bool]) -> bool:
    return any(predicate(path) for path in paths)


def lifecycle_paths() -> set[str]:
    inventory = Path("scripts/ci/lifecycle-scripts.json")
    if not inventory.is_file():
        return set()
    data = json.loads(inventory.read_text())
    return {path for values in data.values() for path in values}


def is_auth_conformance_input(path: str) -> bool:
    """Inputs whose changes must execute the dated auth conformance job."""
    return path in {
        "tools/verification/conformance/auth-requirements.json",
        "tools/verification/conformance/mcp-auth-coverage-manifest.json",
        "tools/verification/conformance/mcp-auth-normative.json",
        "tools/verification/conformance/openai-auth-normative.json",
        "scripts/ci/test_auth_spec_matrix.py",
    } or starts(
        path,
        "scripts/ci/mcp_auth_",
        "scripts/ci/openai-auth-",
        "scripts/ci/refresh_mcp_auth_",
        "scripts/ci/refresh_openai_auth_",
        "scripts/ci/publish_mcp_auth_",
        "scripts/ci/auth_backup_restore_",
        "tools/verification/conformance/mcp-spec-",
        "scripts/ci/mcp_spec_",
        "scripts/ci/mcp_oracle_",
        "scripts/ci/extract_mcp_spec_",
        "scripts/ci/extract_mcp_schema_",
        "scripts/ci/test_mcp_spec_",
        "scripts/ci/test_mcp_oracle_",
        "scripts/ci/test_extract_mcp_spec_",
        "scripts/ci/test_extract_mcp_schema_",
    )


def is_skills_conformance_input(path: str) -> bool:
    """Documentation inputs pinned directly by the accepted Skills contract tests."""
    return path in {
        "docs/contracts/skills-extension.md",
        "docs/seps/2640-skills-extension.mdx",
    }


def is_js_dependency_input(path: str) -> bool:
    return path.endswith(("/package.json", "/package-lock.json", "/pnpm-lock.yaml")) or path in {
        "pnpm-lock.yaml",
        "scripts/ci/js-advisory-policy.json",
        "scripts/ci/js_advisory_gate.py",
        "scripts/ci/test_ci_supply_policy.py",
    }


def is_rust_manifest(path: str) -> bool:
    """Cargo manifests, the toolchain pin, and Cargo configuration."""
    return (
        path == "Cargo.toml"
        or (starts(path, "crates/", "tools/verification/crates/") and path.endswith("/Cargo.toml"))
        or starts(path, ".cargo/")
        or path == "rust-toolchain.toml"
    )


def is_rust_api_input(path: str) -> bool:
    """Rust inputs of rustdoc and the generated catalogs: library, binary,
    example, and build-script sources plus manifests. Integration tests under
    `crates/*/tests/` document nothing and generate no catalog entries."""
    if is_rust_manifest(path) or path in {"Cargo.lock", "Justfile"}:
        return True
    if starts(path, "tools/verification/crates/verify-core/", "tools/verification/crates/verify-scenario/"):
        return "/tests/" not in path
    if not starts(path, "crates/"):
        return False
    parts = path.split("/")
    return len(parts) >= 3 and (parts[2] in {"src", "examples", "build.rs"})


def is_rust_feature_input(path: str) -> bool:
    """Inputs that change which feature slices compile: manifests and the
    slice checker. Ordinary source edits are covered nightly and on main."""
    return is_rust_manifest(path) or path in {
        "scripts/check-feature-slices.sh",
        "clippy.toml",
    }


def is_codemode_input(path: str) -> bool:
    lowered = path.lower()
    return (
        starts(path, "crates/labby-codemode/", "crates/labby/src/cli/gateway/code.rs")
        or (starts(path, "crates/") and ("codemode" in lowered or "code_mode" in lowered))
        or path in TOOLCHAIN_INPUTS
        or path in {"Cargo.toml", "crates/labby/Cargo.toml", "crates/labby-codemode/Cargo.toml"}
    )


def is_mcp_conformance_input(path: str) -> bool:
    """Protocol, authentication, and transport surfaces the dated MCP and auth
    conformance lanes exercise, plus their pinned evidence inputs."""
    return (
        is_auth_conformance_input(path)
        or is_skills_conformance_input(path)
        or starts(
            path,
            "crates/labby/src/mcp/",
            "crates/labby/src/mcp.rs",
            "crates/labby/src/api/",
            "crates/labby/src/api.rs",
            "crates/labby/src/cli/serve",
            "crates/labby-auth/",
            "crates/labby-runtime/",
            "crates/labby-gateway/src/upstream/",
            "crates/labby-gateway/src/upstream.rs",
            "tools/verification/conformance/",
            "scripts/ci/mcp",
            "scripts/ci/test_mcp_",
            "scripts/ci/openai-auth",
        )
        or (starts(path, "crates/labby/tests/") and any(word in path for word in ("mcp", "oauth", "auth", "skills")))
        or path in TOOLCHAIN_INPUTS
        or path in {"Cargo.toml", "crates/labby/Cargo.toml"}
    )


def is_lifecycle_conformance_input(path: str) -> bool:
    """The browser-bridge request lifecycle, the HTTP/WebSocket server that
    hosts it, the gateway, and the verification model that replays it."""
    return (
        starts(
            path,
            "crates/labby-browser/",
            "crates/labby-gateway/",
            "crates/labby-model/",
            "crates/labby/src/api/",
            "crates/labby/src/api.rs",
            "crates/labby/src/dispatch/browser",
            "crates/labby/src/cli/serve",
            "crates/labby/tests/lifecycle_conformance.rs",
            "crates/labby/tests/support/",
            "scripts/ci/validate_conformance_evidence.py",
            "scripts/ci/validate_incident_evidence.py",
            "scripts/ci/test_conformance_workflow.py",
            "scripts/ci/test_incident_evidence.py",
            ".github/workflows/verification-conformance.yml",
        )
        # The isolated verifier replays the captured incidents; the MCP
        # conformance evidence under tools/verification/conformance/ is not
        # an input of this lane.
        or (starts(path, "tools/verification/") and not starts(path, "tools/verification/conformance/"))
        or path in TOOLCHAIN_INPUTS
        or path in {"Cargo.toml", "crates/labby/Cargo.toml"}
    )


def is_installer_input(path: str) -> bool:
    """Inputs of the macOS installer, updater, and recovery contracts."""
    return starts(
        path,
        "install.sh",
        "scripts/install",
        "scripts/package-tailcat-release.sh",
        "scripts/sync-npm-release-version.py",
        "scripts/ci/export-release-attestation-bundles.sh",
        "scripts/tests/installers.sh",
        "scripts/tests/release-attestation-bundles.sh",
        "scripts/tests/tailcat-installers.sh",
        "scripts/tests/test_tailcat_release.py",
        "scripts/tests/test_npm_release_version.py",
        "packages/labby-microsandbox/package.json",
        "crates/labby/src/self_update",
        "crates/labby/tests/gateway_auto_reconnect.rs",
    ) or path == "rust-toolchain.toml"


def classify(event: str, paths: list[str]) -> dict[str, bool]:
    if event in {"schedule", "workflow_dispatch"}:
        return {key: True for key in OUTPUT_KEYS}

    if not paths:
        return {key: True for key in OUTPUT_KEYS}

    workflow = any_match(
        paths,
        lambda p: starts(p, ".github/workflows/", ".github/actions/")
        or p in lifecycle_paths()
        or (starts(p, "scripts/") and p.endswith((".sh", ".ps1")))
        or is_auth_conformance_input(p)
        or starts(p, "scripts/tests/", "scripts/ci/n-minus-one/")
        or p
        in {
            ".github/labeler.yml",
            "Justfile",
            "scripts/ci/test_default_web_build.py",
            "tools/verification/conformance/expected-failures-dated.yaml",
            "tools/verification/conformance/expected-failures-extensions.yaml",
            "scripts/ci/changed_paths.py",
            "scripts/ci/mcp-conformance.sh",
            "scripts/ci/mcp_upstream_drift.py",
            "scripts/ci/test_mcp_upstream_drift.py",
            "tools/verification/conformance/upstream-baseline.json",
            "tools/verification/conformance/auth-requirements.json",
            "tools/verification/conformance/mcp-auth-normative.json",
            "scripts/ci/test_auth_spec_matrix.py",
            "scripts/ci/refresh_mcp_auth_denominator.py",
            "scripts/ci/auth_backup_restore_drill.py",
            "scripts/ci/lifecycle-scripts.json",
            "scripts/ci/check-lifecycle-scripts.sh",
            "scripts/ci/test_release_hardening.py",
            "scripts/sync-npm-release-version.py",
            "scripts/ci/release_channel.py",
            "scripts/ci/release_source.py",
            "scripts/ci/prepare-dev-release.py",
            "scripts/ci/sync-release-version.py",
            "scripts/ci/test_release_channels.py",
            "scripts/ci/test_dev_release_source.py",
            "scripts/ci/dev_release_pr.py",
            "scripts/ci/test_dev_release_pr.py",
            "scripts/ci/test_dev_release_workflow.py",
            ".release-please-dev.json",
            ".github/workflows/release-please.yml",
            "scripts/ci/test_incus_channels.py",

            "scripts/ci/test_release_observer.py",
            "scripts/ci/resolve-n-minus-one-baseline.py",
            "scripts/ci/github-verifier-bootstrap.sh.in",
            "scripts/ci/github-verifier-bootstrap.ps1.in",
            "scripts/ci/github-verifier-bootstrap-pins.json",
            "scripts/ci/generate-verifier-bootstrap.py",
            "scripts/ci/test_release_provenance_bundles.py",
            "scripts/ci/test_verifier_bootstrap.py",
            "scripts/ci/qualify-first-use.py",
            "scripts/ci/first-use-native-driver.py",
            "scripts/ci/test_first_use_qualification.py",
            "scripts/ci/test_first_use_native_driver.py",
            "scripts/ci/check_node_toolchain_sync.py",
            "scripts/ci/js-advisory-policy.json",
            "scripts/ci/js_advisory_gate.py",
            "scripts/ci/test_ci_supply_policy.py",
            "scripts/ci/test_verification_workflow.py",
            "crates/labby/tests/ci_changed_paths.rs",
            "install.sh",
            "scripts/install.sh",
            "scripts/install.ps1",
            "scripts/install-macos-service.sh",
            "apps/web/scripts/sync-install-script.mjs",
            "apps/web/scripts/sync-install-script.test.mjs",
        },
    )
    docs = any_match(
        paths,
        lambda p: starts(p, "docs/")
        or (starts(p, "plugins/labby/", "plugins/install-labby/") and p.endswith(".md"))
        or p in {"README.md", "CHANGELOG.md", "CLAUDE.md", "AGENTS.md", "GEMINI.md"},
    )
    # `just docs-check` validates both generated inventories and local links in
    # maintained Markdown. Canonical prose participates in the check; explicit
    # historical/work-product trees (archive, sessions, superpowers) do not.
    docs_check = any_match(
        paths,
        lambda p: (
            starts(p, "docs/")
            and not starts(p, "docs/archive/", "docs/sessions/", "docs/superpowers/")
        )
        or (starts(p, "plugins/labby/", "plugins/install-labby/") and p.endswith(".md"))
        or starts(p, "plugins/labby/.apm/skills/implement-in-microsandbox/")
        or p
        in {
            ".config/.env.example",
            ".config/config.example.toml",
            "README.md",
            "CLAUDE.md",
            "AGENTS.md",
            "GEMINI.md",
            "crates/labby/tests/ci_changed_paths.rs",
            "scripts/check-doc-links.py",
            "scripts/check-product-docs.py",
            "scripts/ci/changed_paths.py",
            "Justfile",
        },
    )
    native_plugins = any_match(
        paths,
        lambda p: starts(p, "plugins/labby/", "plugins/install-labby/skills/")
        or p in {"scripts/generate-native-plugins.py", "LICENSE", "apm.yml"},
    )
    web = any_match(
        paths,
        lambda p: starts(p, "apps/web/")
        or p in {"Justfile", "scripts/build-web.sh", "scripts/ci/test_default_web_build.py"},
    )
    browser_extension = any_match(
        paths,
        lambda p: starts(
            p,
            "apps/chrome/",
            "crates/labby-browser/",
            "crates/labby/src/api/browser_session.rs",
            "crates/labby/src/api/services/browser.rs",
            "crates/labby/src/dispatch/browser.rs",
            "crates/labby/src/dispatch/browser/",
        ),
    )
    desktop = any_match(paths, lambda p: starts(p, "apps/tauri/"))
    npm = any_match(paths, lambda p: starts(p, "packages/labby-mcp/") or p == "server.json")
    tailcat = any_match(
        paths,
        lambda p: starts(
            p, "packages/labby-tailcat-browser/", "packages/labby-microsandbox/",
            "tools/tailcat-bridge/", "crates/labby-tailcat/",
            "crates/labby/src/dispatch/tailcat/", "crates/labby/src/api/tailcat/",
        ) or p in {
            "crates/labby/src/mcp/tool_execution.rs", "crates/labby/src/mcp/call_tool.rs",
            "crates/labby/src/api/tailcat.rs", "crates/labby/src/dispatch/tailcat.rs",
            "scripts/test-tailcat-bridge.sh", "scripts/build-tailcat-bridge.sh",
            "scripts/package-tailcat-release.sh", "scripts/install-tailcat-depot-assets.sh",
            "scripts/ci/test_tailcat_workflow.py",
        },
    )
    # The M3 model consumes only core/scenario from the isolated toolkit.
    # Their sources and inherited manifest/lints affect product compilation;
    # runner/backend/host code remains on the separate verification lane.
    rust_sources = any_match(
        paths,
        lambda p: starts(
            p,
            "crates/",
            ".cargo/",
            "tools/verification/crates/verify-core/",
            "tools/verification/crates/verify-scenario/",
        )
    )
    rust_manifests = any_match(
        paths,
        lambda p: p
        in {
            "Cargo.toml",
            "Cargo.lock",
            "Justfile",
            "rust-toolchain.toml",
            "build.rs",
            "clippy.toml",
            "deny.toml",
            "tools/verification/Cargo.toml",
            ".config/nextest.toml",
        },
    )
    # `tools/verification/` is a separate Cargo workspace and deliberately matches
    # none of the `rust_sources` prefixes above: a product change should not
    # build the toolkit, and a toolkit change should not run the full product
    # Rust matrix. It therefore needs its own routing key — without one, a
    # change confined to tools/verification/ would route to nothing at all and CI
    # would report green having compiled and tested none of it.
    verification = any_match(
        paths,
        lambda p: starts(p, "tools/verification/", ".cargo/")
        or p in {"rust-toolchain.toml", "clippy.toml", "Justfile"},
    )
    embedded_inputs = any_match(paths, lambda p: starts(
        p, "plugins/labby/.apm/skills/using-labby/", "plugins/labby/.apm/skills/using-codemode/",
        "plugins/labby/.apm/skills/using-snippets/", "docs/contracts/schemas/",
    ) or p == "docs/contracts/skills-extension.md" or (
        p.rpartition("/")[0] == "docs/snippets" and p.endswith((".md", ".js", ".json"))
    ))
    rust_compile = rust_sources or rust_manifests or embedded_inputs
    # Dependency, lockfile, toolchain, and build-policy changes can alter test
    # compilation and runtime behavior just as directly as a Rust source edit.
    rust_test = rust_compile or any_match(paths, lambda p: starts(p, "docs/contracts/fixtures/") or (
        starts(p, "docs/snippets/") and p.endswith((".md", ".test.json"))
    ))
    rust_test = rust_test or any_match(paths, is_auth_conformance_input)
    rust_test = rust_test or any_match(paths, is_skills_conformance_input)
    security = any_match(
        paths,
        lambda p: p in {"Cargo.lock", "deny.toml"} or starts(p, ".cargo/"),
    )
    security = security or rust_sources
    javascript_advisories = any_match(paths, is_js_dependency_input)
    rust_api = any_match(paths, is_rust_api_input)
    # Generated catalogs come from library/binary sources and manifests, not
    # from integration tests, so test-only edits no longer run docs-check.
    docs_check = docs_check or rust_api
    # The worktree-identity readers are exercised by a Python suite in docs-check.
    docs_check = docs_check or any_match(paths, lambda p: starts(
        p, "crates/labby/tests/fixtures/snippets/worktree-identity"
    ) and p.endswith(".py"))
    incus_inputs = any_match(
        paths,
        lambda p: starts(p, ".config/incus/")
        or p in {
            "scripts/incus-bootstrap.sh",
            "scripts/check-incus-ssh",
            "scripts/ci/build-incus-image.sh",
            "scripts/ci/smoke-incus-image.sh",
            "scripts/ci/create-incus-image-manifest.py",
            "scripts/ci/verify-incus-image-manifest.py",
            "scripts/ci/promote-incus-pointer.sh",
            "scripts/ci/test-incus-contract.py",
            "scripts/ci/validate-supply-manifest.py",
        },
    )
    incus = rust_compile or web or incus_inputs
    release = rust_compile or web or any_match(paths, lambda p: starts(p, "release/"))
    unraid = any_match(
        paths,
        lambda p: starts(p, "unraid/")
        or p
        in {
            "scripts/ci/unraid-plugin-checksums.sh",
            "scripts/ci/unraid-runtime-tests.sh",
        },
    )

    result = {
        "all": False,
        "docs": docs,
        "docs_check": docs_check,
        "native_plugins": native_plugins,
        "workflow": workflow,
        "rust_compile": rust_compile,
        "rust_test": rust_test,
        "web": web,
        "browser_extension": browser_extension,
        "desktop": desktop,
        "npm": npm,
        "tailcat": tailcat,
        "incus": incus,
        "security": security,
        "javascript_advisories": javascript_advisories,
        "release": release,
        "unraid": unraid,
        "verification": verification,
        "rust_api": rust_api,
        "rust_features": any_match(paths, is_rust_feature_input),
        "codemode": any_match(paths, is_codemode_input),
        "mcp_conformance": any_match(paths, is_mcp_conformance_input),
        "lifecycle_conformance": any_match(paths, is_lifecycle_conformance_input),
        "installer": any_match(paths, is_installer_input),
        "extended": event in EXTENDED_EVENTS,
    }

    if workflow:
        # ci.yml, the composite actions, and this classifier can affect every
        # job, so those fail closed and enable everything. Other
        # workflow-adjacent files enable only `workflow` plus the categories
        # their own workflows exercise (the release workflows map to the
        # release source contract).
        fail_closed = any_match(
            paths,
            lambda p: starts(p, ".github/actions/")
            or p
            in {
                ".github/workflows/ci.yml",
                "scripts/ci/changed_paths.py",
                "scripts/ci/lifecycle-scripts.json",
                "scripts/ci/check-lifecycle-scripts.sh",
                "scripts/ci/test_release_hardening.py",
                "scripts/ci/test_release_observer.py",
            },
        )
        if fail_closed:
            for key in OUTPUT_KEYS:
                result[key] = True
        elif any_match(
            paths,
            lambda p: p
            in {
                ".github/workflows/release.yml",
                ".github/workflows/dev-release.yml",
                "scripts/sync-npm-release-version.py",
                "scripts/ci/release_channel.py",
                "scripts/ci/release_source.py",
                "scripts/ci/prepare-dev-release.py",
                "scripts/ci/sync-release-version.py",
                "scripts/ci/test_release_channels.py",
                "scripts/ci/test_dev_release_source.py",
                "scripts/ci/dev_release_pr.py",
                "scripts/ci/test_dev_release_pr.py",
                "scripts/ci/test_dev_release_workflow.py",
                ".release-please-dev.json",
                ".github/workflows/release-please.yml",
                "scripts/ci/test_incus_channels.py",
                ".github/workflows/build-incus-image.yml",
                "scripts/ci/resolve-n-minus-one-baseline.py",
                "scripts/ci/github-verifier-bootstrap.sh.in",
                "scripts/ci/github-verifier-bootstrap.ps1.in",
                "scripts/ci/github-verifier-bootstrap-pins.json",
                "scripts/ci/generate-verifier-bootstrap.py",
                "scripts/ci/test_release_provenance_bundles.py",
                "scripts/ci/test_verifier_bootstrap.py",
                "scripts/ci/qualify-first-use.py",
                "scripts/ci/first-use-native-driver.py",
                "scripts/ci/test_first_use_qualification.py",
                "scripts/ci/test_first_use_native_driver.py",
            },
        ):
            result["release"] = True

    return result


def read_paths(path: Path) -> list[str]:
    if not path.exists():
        return []
    return [line.strip() for line in path.read_text().splitlines() if line.strip()]


def git_path_exists(rev: str) -> bool:
    return subprocess.run(
        ["git", "cat-file", "-e", rev],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def git_output(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True, stderr=subprocess.DEVNULL).strip()


def resolve_paths(event: str) -> list[str]:
    if event in {"schedule", "workflow_dispatch"}:
        return []

    env = os.environ
    base = ""
    head = env.get("HEAD_SHA") or env.get("GITHUB_SHA") or "HEAD"

    if event == "pull_request":
        base = env.get("PR_BASE_SHA", "")
        head = env.get("PR_HEAD_SHA") or head
    elif event == "push":
        if env.get("GITHUB_REF", "").startswith("refs/tags/"):
            return []
        base = env.get("PUSH_BEFORE_SHA", "")
    else:
        return []

    if not base or set(base) == {"0"} or not git_path_exists(base):
        try:
            base = git_output("rev-parse", "HEAD^")
        except subprocess.CalledProcessError:
            base = ""

    if not base:
        return []

    try:
        raw = git_output("diff", "--name-only", base, head)
    except subprocess.CalledProcessError:
        return []

    return [line.strip() for line in raw.splitlines() if line.strip()]


def write_outputs(path: Path, values: dict[str, bool]) -> None:
    lines = [f"{key}={'true' if values[key] else 'false'}" for key in OUTPUT_KEYS]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", required=True)
    parser.add_argument("--changed-files", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write-changed-files", type=Path)
    args = parser.parse_args()

    paths = read_paths(args.changed_files) if args.changed_files else resolve_paths(args.event)
    if args.write_changed_files:
        args.write_changed_files.write_text("\n".join(paths) + ("\n" if paths else ""))

    values = classify(args.event, paths)
    write_outputs(args.output, values)
    for key in OUTPUT_KEYS:
        print(f"{key}={str(values[key]).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
