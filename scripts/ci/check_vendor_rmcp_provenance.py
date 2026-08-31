#!/usr/bin/env python3
"""Verify the immutable rmcp base archive and Labby's explicit patch set."""

from __future__ import annotations

import difflib
import hashlib
import json
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "conformance/vendor-rmcp-provenance.json"
VENDOR = ROOT / "vendor/rmcp-3.1.0-labby"
ALLOWED_PACKAGING = {"Cargo.lock", "LICENSE", "README.labby.md"}
TRUSTED_UPSTREAM_REPOSITORY = "https://github.com/dinglebear-ai/rust-sdk"
TRUSTED_ARCHIVE_PREFIX = f"{TRUSTED_UPSTREAM_REPOSITORY}/archive/"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def diff_bytes(upstream: Path, patches: list[dict]) -> bytes:
    chunks = []
    for patch in patches:
        relative = patch["path"]
        before = b"" if patch.get("upstream_absent") else (upstream / relative).read_bytes()
        after = (VENDOR / relative).read_bytes()
        chunks.extend(
            difflib.diff_bytes(
                difflib.unified_diff,
                before.splitlines(keepends=True),
                after.splitlines(keepends=True),
                fromfile=f"a/{relative}".encode(),
                tofile=f"b/{relative}".encode(),
                lineterm=b"\n",
            )
        )
        if before == after:
            raise SystemExit(f"expected a documented diff for {relative}")
    return b"".join(chunks)


def main() -> None:
    manifest = json.loads(MANIFEST.read_text())
    commit = manifest["upstream_commit"]
    if len(commit) != 40 or any(character not in "0123456789abcdef" for character in commit):
        raise SystemExit("vendored rmcp upstream commit must be a full lowercase Git object ID")
    archive_url = f"{TRUSTED_ARCHIVE_PREFIX}{commit}.tar.gz"
    with tempfile.TemporaryDirectory(prefix="labby-rmcp-provenance-") as directory:
        temporary = Path(directory)
        archive = temporary / "upstream.tar.gz"
        archive.write_bytes(urllib.request.urlopen(archive_url, timeout=30).read())
        if sha256(archive) != manifest["upstream_archive_sha256"]:
            raise SystemExit("vendored rmcp upstream archive checksum mismatch")
        with tarfile.open(archive) as source:
            source.extractall(temporary, filter="data")
        checkout = next(temporary.glob("rust-sdk-*"))
        upstream = checkout / manifest["crate_path"]

        patch_paths = {patch["path"] for patch in manifest["patches"]}
        for patch in manifest["patches"]:
            current = VENDOR / patch["path"]
            if sha256(current) != patch["patched_sha256"]:
                raise SystemExit(f"undocumented vendored rmcp patch drift: {patch['path']}")

        observed = set()
        for current in VENDOR.rglob("*"):
            if not current.is_file() or "target" in current.parts:
                continue
            relative = current.relative_to(VENDOR).as_posix()
            base = upstream / relative
            if not base.exists() or current.read_bytes() != base.read_bytes():
                observed.add(relative)
        undocumented = observed - patch_paths - ALLOWED_PACKAGING
        if undocumented:
            raise SystemExit(f"undocumented vendored rmcp files: {sorted(undocumented)}")
        deleted = {
            path.relative_to(upstream).as_posix()
            for path in upstream.rglob("*")
            if path.is_file() and not (VENDOR / path.relative_to(upstream)).exists()
        }
        if deleted:
            raise SystemExit(f"undocumented files deleted from vendored rmcp: {sorted(deleted)}")
        missing = patch_paths - observed
        if missing:
            raise SystemExit(f"declared rmcp patches no longer differ: {sorted(missing)}")

        actual_diff = hashlib.sha256(diff_bytes(upstream, manifest["patches"])).hexdigest()
        if actual_diff != manifest["unified_diff_sha256"]:
            raise SystemExit(
                "vendored rmcp unified patch checksum mismatch: "
                f"expected {manifest['unified_diff_sha256']}, observed {actual_diff}"
            )
    print(f"vendored rmcp provenance passed: {len(manifest['patches'])} explicit patches")


if __name__ == "__main__":
    main()
