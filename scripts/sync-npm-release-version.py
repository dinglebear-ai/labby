#!/usr/bin/env python3
"""Synchronize product npm manifests and lockfile root identities for a release."""
import json
from pathlib import Path
import re
import sys


def sync(root: Path, version: str) -> None:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("invalid release version")
    for name in ("labby-mcp", "labby-microsandbox", "labby-tailcat-browser"):
        directory = root / "packages" / name
        for filename in ("package.json", "package-lock.json"):
            path = directory / filename
            if filename == "package-lock.json" and not path.exists():
                continue
            document = json.loads(path.read_text())
            document["version"] = version
            if filename == "package-lock.json":
                document["packages"][""]["version"] = version
            path.write_text(json.dumps(document, indent=2) + "\n")


if __name__ == "__main__":
    sync(Path(sys.argv[1]), sys.argv[2])
