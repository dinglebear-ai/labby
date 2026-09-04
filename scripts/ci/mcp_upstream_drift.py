#!/usr/bin/env python3
"""Report MCP specification/rmcp drift and map it to Labby ownership."""

from __future__ import annotations

import argparse
import json
import os
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class Ownership:
    keywords: tuple[str, ...]
    local_paths: tuple[str, ...]
    checks: tuple[str, ...]


OWNERSHIP = (
    Ownership(
        ("authorization", "oauth", "security-considerations", "client-registration"),
        ("crates/labby-auth/src/", "crates/labby/src/api/router.rs"),
        ("cargo test -p labby-auth --all-features --locked",),
    ),
    Ownership(
        ("transport", "streamable-http", "stateless", "session"),
        ("crates/labby/src/cli/serve.rs", "crates/labby/src/mcp/", "crates/labby-gateway/src/"),
        ("cargo test -p labby --all-features --locked cli::serve::tests::http_mcp_",),
    ),
    Ownership(
        ("discover", "lifecycle", "versioning", "initialize"),
        ("crates/labby/src/mcp/server.rs", "crates/labby/src/cli/serve.rs"),
        ("cargo test -p labby --all-features --locked mcp::server::tests",),
    ),
    Ownership(
        ("task", "mrtr", "elicitation", "input_required"),
        (
            "crates/labby/src/mcp/call_tool.rs",
            "crates/labby/src/mcp/call_tool_upstream.rs",
            "crates/labby-gateway/src/",
        ),
        ("scripts/ci/mcp-conformance.sh",),
    ),
    Ownership(
        ("tool", "resource", "prompt", "completion", "schema"),
        ("crates/labby/src/mcp/handlers_", "crates/labby/src/mcp/catalog.rs"),
        ("cargo test -p labby --all-features --locked mcp::handlers_",),
    ),
    Ownership(
        ("caching", "subscription", "notification", "event-store"),
        ("crates/labby/src/mcp/peers.rs", "crates/labby/src/mcp/resource_proxy.rs"),
        ("cargo test -p labby --all-features --locked stateless_subscription_",),
    ),
    Ownership(
        ("extension", "apps", "ui"),
        ("crates/labby/src/mcp/server.rs", "crates/labby/src/mcp/handlers_resources.rs"),
        ("cargo test -p labby --all-features --locked mcp::server::tests",),
    ),
    Ownership(
        ("2640-skills-extension", "skills-extension", "skills/list", "skills/get"),
        (
            "docs/contracts/skills-extension.md",
            "crates/labby-runtime/src/skills/",
            "crates/labby-gateway/src/upstream/pool/skills.rs",
            "crates/labby/src/skills/",
            "crates/labby/src/mcp/skills.rs",
        ),
        (
            "cargo test -p labby-runtime --all-features --locked --test skills_contract_conformance",
            "cargo test -p labby-gateway --all-features --locked upstream::pool::skills_tests",
            "cargo test -p labby --all-features --locked skills",
        ),
    ),
)

DEFAULT_PATHS = (
    "Cargo.toml",
    "scripts/ci/mcp-conformance.sh",
    "docs/surfaces/MCP_CONFORMANCE.md",
)
DEFAULT_CHECKS = ("scripts/ci/mcp-conformance.sh",)


def api_json(url: str, token: str | None) -> Any:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "labby-mcp-drift-watch",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as response:
        return json.load(response)


def changed_files(compare: dict[str, Any]) -> list[str]:
    return [entry["filename"] for entry in compare.get("files", [])]


def map_ownership(paths: list[str], release_text: str = "") -> tuple[list[str], list[str]]:
    haystack = "\n".join(paths + [release_text]).lower()
    local_paths: set[str] = set(DEFAULT_PATHS)
    checks: set[str] = set(DEFAULT_CHECKS)
    for owner in OWNERSHIP:
        if any(keyword in haystack for keyword in owner.keywords):
            local_paths.update(owner.local_paths)
            checks.update(owner.checks)
    return sorted(local_paths), sorted(checks)


def compare_url(repository: str, before: str, after: str) -> str:
    return f"https://api.github.com/repos/{repository}/compare/{before}...{after}"


def generate_report(baseline: dict[str, Any], token: str | None) -> tuple[str, bool]:
    spec = baseline["mcp_spec"]
    rmcp = baseline["rmcp"]
    spec_head = api_json(
        f"https://api.github.com/repos/{spec['repository']}/commits/{spec['ref']}", token
    )["sha"]
    releases = api_json(
        f"https://api.github.com/repos/{rmcp['repository']}/releases?per_page=20", token
    )
    latest_release = next(release for release in releases if not release["draft"])
    latest_tag = latest_release["tag_name"]
    latest_commit = api_json(
        f"https://api.github.com/repos/{rmcp['repository']}/commits/{latest_tag}", token
    )["sha"]

    spec_compare = api_json(
        compare_url(spec["repository"], spec["commit"], spec_head), token
    )
    rmcp_compare = api_json(
        compare_url(rmcp["repository"], rmcp["commit"], latest_commit), token
    )
    spec_files = changed_files(spec_compare)
    rmcp_files = changed_files(rmcp_compare)

    # SEP-2640 is accepted, but remains on its canonical SEP branch until it is
    # folded into a dated specification release. Watch only the normative file;
    # the ext-skills working-group repository is implementation guidance.
    skills = baseline.get("skills_extension")
    skills_head = None
    skills_files: list[str] = []
    if skills:
        skills_head = api_json(
            f"https://api.github.com/repos/{skills['repository']}/commits/{skills['ref']}",
            token,
        )["sha"]
        if skills_head != skills["commit"]:
            skills_compare = api_json(
                compare_url(skills["repository"], skills["commit"], skills_head), token
            )
            watched = set(skills.get("watched_paths", ()))
            skills_files = [
                path for path in changed_files(skills_compare) if path in watched
            ]

    drift = (
        spec_head != spec["commit"]
        or latest_commit != rmcp["commit"]
        or bool(skills_files)
    )
    mapped_paths, checks = map_ownership(
        spec_files + rmcp_files + skills_files, latest_release.get("body") or ""
    )

    lines = [
        "# MCP upstream drift report",
        "",
        "<!-- labby-mcp-upstream-drift -->",
        "",
        f"**Drift detected:** {'yes' if drift else 'no'}",
        "",
        "## Baselines and current upstream",
        "",
        "| Surface | Baseline | Current |",
        "|---|---|---|",
        f"| MCP spec `{spec['protocol_version']}` | `{spec['commit']}` | `{spec_head}` |",
        f"| rmcp `{rmcp['crate_version']}` | `{rmcp['commit']}` / `{rmcp['release_tag']}` | `{latest_commit}` / `{latest_tag}` |",
        *(
            [f"| Skills extension (accepted SEP-2640) | `{skills['commit']}` | `{skills_head}` |"]
            if skills
            else []
        ),
        "",
        "## Upstream files changed",
        "",
        "### MCP specification",
        *([f"- `{path}`" for path in spec_files] or ["- None"]),
        "",
        "### rmcp",
        *([f"- `{path}`" for path in rmcp_files] or ["- None"]),
        "",
        *(
            [
                "### Skills extension (accepted SEP-2640)",
                *([f"- `{path}`" for path in skills_files] or ["- None (normative documents unchanged)"]),
                "",
                "Labby implements the accepted SEP at a pinned canonical revision. When its",
                f"normative document moves, re-read it, update `{skills['contract']}` and the",
                "conformance fixtures it binds, then advance the baseline in the same PR.",
                "",
            ]
            if skills
            else []
        ),
        "## Labby code that must be reviewed",
        "",
        *[f"- `{path}`" for path in mapped_paths],
        "",
        "## Required validation",
        "",
        *[f"- `{check}`" for check in checks],
        *(
            ["- `cargo test -p labby-runtime --all-features --locked skills_contract_conformance`"]
            if skills_files
            else []
        ),
        "",
        "When the upstream change is intentionally adopted, update code/tests first, run the",
        "listed validation, then advance `conformance/upstream-baseline.json` in the same PR.",
    ]
    return "\n".join(lines) + "\n", drift


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--baseline", type=Path, default=Path("conformance/upstream-baseline.json")
    )
    parser.add_argument("--output", type=Path, default=Path("target/mcp-upstream-drift.md"))
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    report, drift = generate_report(
        json.loads(args.baseline.read_text()), os.environ.get("GITHUB_TOKEN")
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(report)
    print(report, end="")
    if args.github_output:
        with args.github_output.open("a") as output:
            output.write(f"drift={'true' if drift else 'false'}\n")
            output.write(f"report={args.output}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
