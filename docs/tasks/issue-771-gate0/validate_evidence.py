#!/usr/bin/env python3
"""Validate Gate 0 receipts and the docs-only change boundary; no product assurance."""
from __future__ import annotations

import ast
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BASE = "b42818f256968466c7d85828aa42c1cd8523efad"
OWNED = "docs/tasks/issue-771-gate0/"
ADR = "docs/adr/0007-durable-mcp-task-routing.md"


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main() -> None:
    for name in ("capture.py", "validate_evidence.py", "preserve_conformance.py"):
        ast.parse((HERE / name).read_text(), filename=name)
    sources = json.loads((HERE / "issue-sources.json").read_text())
    assert {x["result"]["number"] for x in sources["snapshots"]} == {208, 709, 771}
    current_capture_hash = hashlib.sha256((HERE / "capture.py").read_bytes()).hexdigest()
    receipts = sorted((HERE / "evidence").glob("*.json"))
    recorded_origin = json.loads((HERE / "evidence" / "s01-identity.json").read_text())
    counts = {"completed_zero_exit": 0, "completed_nonzero_exit": 0, "capture_stopped": 0}
    for path in receipts:
        item = json.loads(path.read_text())
        assert item["label"] == path.stem, path
        assert item["head"] == BASE, path
        assert item["cwd"] == recorded_origin["cwd"], path
        assert item["log"] == path.with_suffix(".log").name, path
        payload = path.with_suffix(".log").read_bytes()
        assert len(payload) == item["log_bytes"], path
        assert hashlib.sha256(payload).hexdigest() == item["log_sha256"], path
        if "capture_sha256" in item:
            assert item["capture_sha256"] == current_capture_hash, path
        kind = "capture_stopped" if item["stopped_by_capture"] else (
            "completed_zero_exit" if item["exit_code"] == 0 else "completed_nonzero_exit")
        counts[kind] += 1
    manifest = json.loads((HERE / "evidence/conformance/manifest.json").read_text())
    archive = HERE / "evidence/conformance" / manifest["archive"]
    assert archive.stat().st_size == manifest["archive_bytes"]
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == manifest["archive_sha256"]
    expected = {entry["path"]: entry for entry in manifest["files"]}
    assert len(expected) == len(manifest["files"])
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        assert {entry.name for entry in members} == set(expected)
        assert len(members) == len(expected)
        for member in members:
            assert member.isfile() and not member.name.startswith("/")
            assert ".." not in Path(member.name).parts
            assert member.size == expected[member.name]["bytes"]
            stream = tar.extractfile(member)
            assert stream is not None
            with stream:
                payload = stream.read()
            assert hashlib.sha256(payload).hexdigest() == expected[member.name]["sha256"]
    assert sum(entry["bytes"] for entry in expected.values()) == manifest["raw_bytes"]
    print("Conformance archive and every raw member match their recorded hashes.")
    names = {path.stem for path in receipts}
    assert {"s01-identity", "s01-sdk-direct-diff", "s01-proposal", "s02-conformance", "s02-harness-tests"} <= names
    changed = git("diff", "--name-only", "HEAD").splitlines()
    changed += git("ls-files", "--others", "--exclude-standard").splitlines()
    assert all(p.startswith(OWNED) or p in (ADR, "docs/adr/README.md") for p in changed), changed
    git("diff", "--check", "HEAD")
    print(json.dumps({"receipt_count": len(receipts), "log_hashes": "all match", "source_json": "valid",
                      "helper_syntax": "valid", "change_boundary": "Gate 0 docs/evidence only",
                      "counts_are_commands_not_tests": counts}, indent=2))
    print("Only completed receipts are validated; a currently running capture has no receipt yet.")


if __name__ == "__main__":
    main()
