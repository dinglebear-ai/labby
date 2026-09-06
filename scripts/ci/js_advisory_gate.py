#!/usr/bin/env python3
"""Inventory and audit every committed JavaScript dependency graph."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
from pathlib import Path

SEVERITIES = {"low": 1, "moderate": 2, "medium": 2, "high": 3, "critical": 4}


def load_policy(path: Path) -> dict:
    policy = json.loads(path.read_text())
    if policy.get("schema_version") != 1 or policy.get("minimum_severity") not in SEVERITIES:
        raise ValueError("unsupported JavaScript advisory policy")
    today = dt.date.today()
    for ignored in policy.get("ignored_advisories", []):
        if not all(isinstance(ignored.get(key), str) and ignored[key] for key in ("id", "rationale", "expires")):
            raise ValueError("every ignored advisory requires id, rationale, and expires")
        if dt.date.fromisoformat(ignored["expires"]) < today:
            raise ValueError(f"ignored advisory {ignored['id']} expired on {ignored['expires']}")
    return policy


def tracked_dependency_files(root: Path) -> set[str]:
    result = subprocess.run(
        ["git", "ls-files", "*package.json", "*package-lock.json", "*pnpm-lock.yaml"],
        cwd=root, check=True, capture_output=True, text=True,
    )
    return {line for line in result.stdout.splitlines() if line}


def validate_inventory(root: Path, policy: dict) -> list[str]:
    errors = []
    declared = []
    tracked = tracked_dependency_files(root)
    for workspace in policy["workspaces"]:
        for key in ("manifest", "lockfile"):
            path = workspace.get(key)
            if not isinstance(path, str) or not (root / path).is_file():
                errors.append(f"missing declared {key}: {path}")
            else:
                declared.append(path)
        if workspace.get("manager") not in {"npm", "pnpm"}:
            errors.append(f"unsupported manager: {workspace.get('manager')}")
    duplicates = sorted({path for path in declared if declared.count(path) > 1})
    if duplicates:
        errors.append(f"dependency files declared more than once: {', '.join(duplicates)}")
    missing = sorted(tracked - set(declared))
    extra = sorted(set(declared) - tracked)
    if missing:
        errors.append(f"uncovered committed dependency files: {', '.join(missing)}")
    if extra:
        errors.append(f"declared dependency files are not committed: {', '.join(extra)}")
    return errors


def advisory_records(payload: dict) -> list[dict]:
    records = []
    for name, vulnerability in payload.get("vulnerabilities", {}).items():
        severity = vulnerability.get("severity", "unknown")
        vias = vulnerability.get("via", [])
        detailed = [via for via in vias if isinstance(via, dict)] or [{}]
        for via in detailed:
            records.append({
                "id": str(via.get("source", via.get("url", name))),
                "package": name,
                "severity": via.get("severity", severity),
                "url": via.get("url", ""),
                "range": via.get("range", vulnerability.get("range", "")),
                "remediation": "upgrade the package outside the vulnerable range",
            })
    for advisory_id, advisory in payload.get("advisories", {}).items():
        records.append({
            "id": str(advisory_id),
            "package": advisory.get("module_name", "unknown"),
            "severity": advisory.get("severity", "unknown"),
            "url": advisory.get("url", ""),
            "range": advisory.get("vulnerable_versions", ""),
            "remediation": "upgrade the package outside the vulnerable range",
        })
    return records


def blocking_advisories(payload: dict, policy: dict) -> list[dict]:
    minimum = SEVERITIES[policy["minimum_severity"]]
    ignored = {entry["id"] for entry in policy.get("ignored_advisories", [])}
    return [
        record for record in advisory_records(payload)
        if SEVERITIES.get(str(record["severity"]).lower(), 0) >= minimum and record["id"] not in ignored
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--policy", type=Path, default=Path("scripts/ci/js-advisory-policy.json"))
    parser.add_argument("--check-inventory", action="store_true")
    args = parser.parse_args()
    root = Path.cwd()
    try:
        policy = load_policy(args.policy)
        errors = validate_inventory(root, policy)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"::error::{error}")
        return 1
    if errors:
        print("\n".join(f"::error::{error}" for error in errors))
        return 1
    if args.check_inventory:
        print("Every committed JavaScript manifest and lockfile has exactly one advisory owner.")
        return 0

    failures = []
    for workspace in policy["workspaces"]:
        directory = str(Path(workspace["manifest"]).parent)
        command = [workspace["manager"], "audit", "--json"]
        completed = subprocess.run(command, cwd=root / directory, capture_output=True, text=True)
        try:
            payload = json.loads(completed.stdout)
        except json.JSONDecodeError:
            print(completed.stderr, file=sys.stderr)
            failures.append({"workspace": directory, "error": "audit did not return JSON"})
            continue
        records = advisory_records(payload)
        for advisory in blocking_advisories(payload, policy):
            failures.append({"workspace": directory, **advisory})
        if completed.returncode and not records:
            failures.append({"workspace": directory, "error": payload.get("error", payload)})
    if failures:
        print(json.dumps({"status": "blocked", "findings": failures}, indent=2))
        return 1
    print(json.dumps({"status": "passed", "minimum_severity": policy["minimum_severity"], "ignored": policy["ignored_advisories"]}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
