#!/usr/bin/env python3
"""Validate the production MCP SDK source independently of TOML key ordering."""

import argparse
import re
from pathlib import Path
import tomllib


def matches_pin(manifest: dict, repository: str, revision: str) -> bool:
    try:
        dependency = manifest["workspace"]["dependencies"]["rmcp"]
    except (KeyError, TypeError):
        return False
    return (
        isinstance(dependency, dict)
        and dependency.get("git") == repository
        and dependency.get("rev") == revision
        and "branch" not in dependency
        and "tag" not in dependency
        and "path" not in dependency
    )


def matches_auth_pin(manifest: dict, repository: str, revision: str, version: str) -> bool:
    """The reusable auth crate keeps its explicit client-only dependency."""
    dependency = manifest.get("dependencies", {}).get("rmcp-client", {})
    return (
        isinstance(version, str)
        and re.fullmatch(r"=[0-9]+\.[0-9]+\.[0-9]+", version) is not None
        and isinstance(dependency, dict)
        and dependency.get("package") == "rmcp"
        and matches_pin({"workspace": {"dependencies": {"rmcp": dependency}}}, repository, revision)
        and dependency.get("version") == version
        and dependency.get("default-features") is False
        and dependency.get("optional") is True
        and isinstance(dependency.get("features"), list)
        and all(isinstance(feature, str) for feature in dependency["features"])
        and set(dependency["features"]) == {"client", "auth", "transport-streamable-http-client-reqwest"}
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("repository")
    parser.add_argument("revision")
    args = parser.parse_args()
    try:
        with args.manifest.open("rb") as source:
            manifest = tomllib.load(source)
        valid = matches_pin(manifest, args.repository, args.revision)
        auth_path = args.manifest.parent / "crates/labby-auth/Cargo.toml"
        with auth_path.open("rb") as source:
            auth_manifest = tomllib.load(source)
        version = manifest.get("workspace", {}).get("dependencies", {}).get("rmcp", {}).get("version")
        valid = valid and matches_auth_pin(auth_manifest, args.repository, args.revision, version)
    except (OSError, tomllib.TOMLDecodeError):
        valid = False
    if not valid:
        parser.exit(1, "Cargo.toml and labby-auth must use the configured immutable rmcp git revision and matching version; auth must remain client-only\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
