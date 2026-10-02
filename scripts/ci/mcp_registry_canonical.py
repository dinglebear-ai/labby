#!/usr/bin/env python3
"""Hash MCP manifests as the Registry serves them.

The Registry omits optional argument/env flags set to false. Treat those
omissions as their schema defaults when comparing the submitted manifest with
the public response.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys


DEFAULT_FALSE_FIELDS = frozenset({"isRequired", "isSecret", "isRepeated"})


def normalized(value: object) -> object:
    if isinstance(value, dict):
        return {
            key: normalized(item)
            for key, item in value.items()
            if not (key in DEFAULT_FALSE_FIELDS and item is False)
        }
    if isinstance(value, list):
        return [normalized(item) for item in value]
    return value


def manifest_sha256(manifest: dict) -> str:
    canonical = json.dumps(normalized(manifest), sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()


def matches_legacy_manifest(source: dict, served: dict, expected_digest: str) -> bool:
    """Bind legacy raw hashes to the exact source before schema normalization."""
    raw = json.dumps(source, sort_keys=True, separators=(",", ":"))
    return (hashlib.sha256(raw.encode()).hexdigest() == expected_digest
            and manifest_sha256(source) == manifest_sha256(served))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: mcp_registry_canonical.py <server.json>")
    print(manifest_sha256(json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))))
