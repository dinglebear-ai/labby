#!/usr/bin/env python3
"""Preserve exact Gate 0 conformance artifacts in a bounded, hashed archive."""
from __future__ import annotations

from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import tarfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def main() -> None:
    source = ROOT / "target/mcp-conformance-gate0"
    destination = HERE / "evidence/conformance"
    assert source.is_dir(), source
    assert not destination.exists(), "Do not overwrite retained evidence"
    receipt = json.loads((HERE / "evidence/s02-conformance-r2.json").read_text())
    assert receipt["exit_code"] == 0 and receipt["stopped_by_capture"] is None
    files = sorted(p for p in source.rglob("*") if p.is_file())
    assert files and not any(p.is_symlink() for p in files)
    assert sum(p.stat().st_size for p in files) <= 8 * 1024**2
    entries = []
    summaries = {}
    for path in files:
        relative = path.relative_to(source)
        payload = path.read_bytes()
        entries.append({"path": relative.as_posix(), "bytes": len(payload),
                        "sha256": hashlib.sha256(payload).hexdigest()})
        if path.name == "checks.json":
            group = relative.parts[0]
            checks = json.loads(payload)
            assert isinstance(checks, list)
            item = summaries.setdefault(group, {"scenario_reports": 0, "statuses": Counter(), "non_success_checks": []})
            item["scenario_reports"] += 1
            item["statuses"].update(check["status"] for check in checks)
            item["non_success_checks"].extend(
                {"report": relative.as_posix(), "id": check["id"], "status": check["status"]}
                for check in checks if check["status"] not in ("SUCCESS", "INFO"))
    assert "Labby multi-hop conformance passed" in (source / "labby-multihop.log").read_text()
    direct = json.loads((source / "direct-proxy.json").read_text())
    assert direct["result"] == "passed" and direct["cleanup"] == "passed"
    destination.mkdir()
    archive = destination / "reports.tar.gz"
    with archive.open("xb") as output, gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as tar:
            for path in files:
                info = tar.gettarinfo(str(path), arcname=path.relative_to(source).as_posix())
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = 0
                with path.open("rb") as payload:
                    tar.addfile(info, payload)
    manifest = {"source_command_receipt": "../s02-conformance-r2.json", "source_head": receipt["head"],
                "source_directory": str(source), "archive": archive.name,
                "archive_bytes": archive.stat().st_size,
                "archive_sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                "raw_bytes": sum(item["bytes"] for item in entries), "files": entries,
                "scenario_summaries": summaries, "multi_hop": "passed", "direct_proxy": direct}
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({key: manifest[key] for key in ("archive_bytes", "raw_bytes", "scenario_summaries", "multi_hop", "direct_proxy")}, indent=2))


if __name__ == "__main__":
    main()
