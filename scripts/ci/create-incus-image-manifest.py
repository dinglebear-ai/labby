#!/usr/bin/env python3
"""Record the exact independently published Incus image artifacts."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--tag", required=True)
parser.add_argument("--repository", required=True)
parser.add_argument("--commit", required=True)
parser.add_argument("--directory", type=Path, required=True)
args = parser.parse_args()

if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
    raise SystemExit("image source commit must be a full Git SHA")
if args.tag != f"incus-{args.commit}":
    raise SystemExit("image tag must identify its exact source commit")

directory = args.directory
image = directory / "labby-incus-x86_64-unknown-linux-gnu.tar.xz"
checksum = directory / f"{image.name}.sha256"
sums = directory / "SHA256SUMS"
for path in (image, checksum, sums):
    if not path.is_file():
        raise SystemExit(f"missing image artifact: {path}")

def entry(path: Path) -> dict[str, object]:
    return {
        "name": path.name,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "size": path.stat().st_size,
    }

payload = {
    "schema": "ai.dinglebear.labby/incus-image-manifest/v1",
    "repository": args.repository,
    "tag": args.tag,
    "commit": args.commit,
    "assets": [entry(path) for path in sorted(directory.iterdir()) if path.is_file()],
}
(directory / "incus-image-manifest.json").write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
