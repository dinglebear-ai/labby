#!/usr/bin/env python3
"""Merge independent release rollback outcomes into one durable record."""
import argparse, json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("--image-record", type=Path, required=True)
p.add_argument("--image-rc", type=int, required=True)
p.add_argument("--pointer-rc", type=int, required=True)
p.add_argument("--github-rc", type=int, required=True)
p.add_argument("--npm-candidate-published", choices=("true", "false"), required=True)
p.add_argument("--mcp-version-published", choices=("true", "false"), required=True)
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
try:
    image = json.loads(a.image_record.read_text())
except Exception as exc:
    image = {"status": "failed", "error": f"image rollback record unavailable: {exc}"}
npm_published = a.npm_candidate_published == "true"
mcp_published = a.mcp_version_published == "true"
all_ok = (
    a.image_rc == a.pointer_rc == a.github_rc == 0
    and image.get("status") == "ok"
    and not npm_published
    and not mcp_published
)
payload = {
    "status": "ok" if all_ok else "failed",
    "image_registry": image,
    "incus_pointer": {"status": "ok" if a.pointer_rc == 0 else "failed", "exit_code": a.pointer_rc},
    "github_release": {"status": "ok" if a.github_rc == 0 else "failed", "exit_code": a.github_rc},
    "npm_candidate": {"status": "manual_reconciliation_required" if npm_published else "not_published"},
    "mcp_version": {"status": "manual_reconciliation_required" if mcp_published else "not_published"},
}
a.output.write_text(json.dumps(payload, sort_keys=True) + "\n")
print(json.dumps(payload, sort_keys=True))
raise SystemExit(0 if payload["status"] == "ok" else 1)
