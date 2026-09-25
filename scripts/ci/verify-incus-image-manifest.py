#!/usr/bin/env python3
"""Fail closed if an image release differs from its immutable manifest."""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
manifest = json.loads((root / "incus-image-manifest.json").read_text())
if manifest.get("schema") != "ai.dinglebear.labby/incus-image-manifest/v1":
    raise SystemExit("unsupported Incus image manifest")
if manifest.get("tag") != f'incus-{manifest.get("commit", "")}':
    raise SystemExit("image tag does not match source commit")
expected = {row["name"] for row in manifest["assets"]} | {"incus-image-manifest.json"}
actual = {path.name for path in root.iterdir() if path.is_file()} - {"generation.json"}
if actual != expected:
    raise SystemExit(f"image asset inventory mismatch: {sorted(actual ^ expected)}")
for row in manifest["assets"]:
    path = root / row["name"]
    if path.stat().st_size != row["size"] or hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
        raise SystemExit(f"image asset digest mismatch: {row['name']}")
