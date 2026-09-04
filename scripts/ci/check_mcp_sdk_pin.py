#!/usr/bin/env python3
"""Validate the production MCP SDK source independently of TOML key ordering."""

import argparse
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
    except (OSError, tomllib.TOMLDecodeError):
        valid = False
    if not valid:
        parser.exit(1, "Cargo.toml must use the configured immutable rmcp git revision\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
