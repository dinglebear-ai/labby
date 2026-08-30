#!/usr/bin/env python3
"""Publish explicit assertion-level MCP authorization coverage mappings."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "conformance/mcp-auth-normative.json"
MANIFEST = ROOT / "conformance/mcp-auth-coverage-manifest.json"
SUMMARY = ROOT / "conformance/auth-requirements.json"
HARNESS = "python3 scripts/ci/mcp_auth_normative_conformance.py"
SUMMARY_ROWS = {
    "MCP-AUTH-001": "MCP-2026-AUTH-INDEX-004",
    "MCP-AUTH-002": "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-001",
    "MCP-AUTH-003": "MCP-2026-AUTH-AUTHORIZATION-SERVER-DISCOVERY-005",
    "MCP-AUTH-004": "MCP-2026-AUTH-INDEX-011",
    "MCP-AUTH-005": "MCP-2026-AUTH-INDEX-012",
    "MCP-AUTH-006": "MCP-2026-AUTH-INDEX-019",
    "MCP-AUTH-007": "MCP-2026-AUTH-INDEX-027",
    "MCP-AUTH-008": "MCP-2026-AUTH-INDEX-035",
    "MCP-AUTH-009": "MCP-2026-AUTH-INDEX-039",
    "MCP-AUTH-010": "MCP-2026-AUTH-INDEX-046",
    "MCP-AUTH-011": "MCP-2026-AUTH-INDEX-050",
    "MCP-AUTH-012": "MCP-2026-AUTH-INDEX-052",
    "MCP-AUTH-013": "MCP-2026-AUTH-INDEX-015",
    "MCP-AUTH-014": "MCP-2026-AUTH-INDEX-061",
    "MCP-AUTH-015": "MCP-2026-AUTH-INDEX-002",
}


def main() -> None:
    data = json.loads(MATRIX.read_text())
    coverage = json.loads(MANIFEST.read_text())
    if coverage["protocol_version"] != data["protocol_version"]:
        raise SystemExit("MCP coverage manifest protocol version mismatch")
    mappings = {entry["row_id"]: entry for entry in coverage["coverage"]}
    row_ids = {row["id"] for row in data["requirements"]}
    if set(mappings) != row_ids or len(mappings) != len(coverage["coverage"]):
        raise SystemExit("MCP coverage manifest must map every row exactly once")
    for row in data["requirements"]:
        entry = mappings[row["id"]]
        digest = hashlib.sha256(row["requirement"].encode()).hexdigest()
        if digest != entry["source_requirement_sha256"]:
            raise SystemExit(f"stale assertion mapping for {row['id']}")
        if not entry["asserted_obligation"] or not entry["assertion_test_ids"]:
            raise SystemExit(f"empty assertion mapping for {row['id']}")
        row["implementation"] = entry["implementation"]
        row["evidence_paths"] = entry["evidence_paths"]
        row["test_id"] = entry["assertion_test_ids"][0]
        row["assertion_test_ids"] = entry["assertion_test_ids"]
        row["asserted_obligation"] = entry["asserted_obligation"]
        row["verification_commands"] = [f"{HARNESS} {row['id']}"]
        row["applicability"] = entry["applicability"]
        row["status"] = entry["status"]
    MATRIX.write_text(json.dumps(data, indent=2) + "\n")
    by_id = {row["id"]: row for row in data["requirements"]}
    summary = json.loads(SUMMARY.read_text())
    for row in summary["requirements"]:
        if row["id"] in SUMMARY_ROWS:
            normative = by_id[SUMMARY_ROWS[row["id"]]]
            row["implementation"] = normative["implementation"]
            row["evidence_paths"] = normative["evidence_paths"]
            row["test_id"] = normative["test_id"]
            row["verification_commands"] = normative["verification_commands"]
            row["status"] = "not_applicable" if row["id"] == "MCP-AUTH-015" else normative["status"]
        elif row["id"] == "MCP-AUTH-016":
            row["implementation"] = "Public metadata and route registration omit DCR together when disabled."
            row["evidence_paths"] = ["crates/labby/src/api/router.rs"]
            row["test_id"] = "api::router::tests::disabled_dynamic_registration_is_neither_advertised_nor_mounted"
            row["verification_commands"] = ["scripts/ci/openai-auth-conformance.sh OAI-AUTH-009"]
            row["status"] = "pass"
    SUMMARY.write_text(json.dumps(summary, indent=2) + "\n")
    print(f"published {len(data['requirements'])} explicit MCP assertions and reconciled the curated summary")


if __name__ == "__main__":
    main()
