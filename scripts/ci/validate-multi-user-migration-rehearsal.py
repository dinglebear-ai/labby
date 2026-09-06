#!/usr/bin/env python3
"""Validate fail-closed Labby/Depot ownership-migration rehearsal evidence."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any

SHA256 = re.compile(r"[0-9a-f]{64}")

REQUIRED_INVENTORIES = {
    "labby": {
        "access_metadata",
        "organizations",
        "principals",
        "principal_links",
        "projects",
        "project_memberships",
        "project_loadouts",
        "project_policy_publications",
        "access_audit",
        "access_admission_buckets",
        "access_security_events",
    },
    "depot": {
        "skills",
        "origins",
        "bundles",
        "cas",
        "tokens",
        "secrets",
        "sources",
        "jobs",
        "job_inputs",
        "uploads",
        "artifacts",
        "artifact_candidates",
    },
}


def fail(message: str) -> None:
    raise ValueError(message)


def digest(value: Any, path: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        fail(f"{path} must be a lowercase SHA-256 digest")
    return value


def count(value: Any, path: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        fail(f"{path} must be a non-negative integer")
    return value


def snapshot(value: Any, path: str) -> tuple[int, str, str]:
    if not isinstance(value, dict):
        fail(f"{path} must be an object")
    if set(value) != {"count", "stableIdsSha256", "contentSha256"}:
        fail(f"{path} must contain count, stableIdsSha256, and contentSha256 exactly")
    return (
        count(value["count"], f"{path}.count"),
        digest(value["stableIdsSha256"], f"{path}.stableIdsSha256"),
        digest(value["contentSha256"], f"{path}.contentSha256"),
    )


def validate_inventory(system: str, value: Any) -> None:
    path = f"systems.{system}.inventory"
    if not isinstance(value, list):
        fail(f"{path} must be an array")
    rows: dict[str, dict[str, Any]] = {}
    for offset, row in enumerate(value):
        row_path = f"{path}[{offset}]"
        if not isinstance(row, dict):
            fail(f"{row_path} must be an object")
        required = {"class", "pre", "post", "expected"}
        if set(row) != required:
            fail(f"{row_path} must contain class, pre, post, and expected exactly")
        name = row["class"]
        if not isinstance(name, str) or not name:
            fail(f"{row_path}.class must be a non-empty string")
        if name in rows:
            fail(f"{path} contains duplicate class {name}")
        rows[name] = row

    expected_classes = REQUIRED_INVENTORIES[system]
    actual_classes = set(rows)
    if actual_classes != expected_classes:
        fail(
            f"{path} class mismatch; missing={sorted(expected_classes - actual_classes)} "
            f"unexpected={sorted(actual_classes - expected_classes)}"
        )

    for name, row in rows.items():
        row_path = f"{path}.{name}"
        pre_count, pre_ids, pre_content = snapshot(row["pre"], f"{row_path}.pre")
        post_count, post_ids, post_content = snapshot(row["post"], f"{row_path}.post")
        expected = row["expected"]
        expected_keys = {
            "countDelta",
            "preserveStableIds",
            "preserveContent",
            "quarantineCount",
        }
        if not isinstance(expected, dict) or set(expected) != expected_keys:
            fail(f"{row_path}.expected has an incomplete expectation set")
        delta = expected["countDelta"]
        if not isinstance(delta, int) or isinstance(delta, bool):
            fail(f"{row_path}.expected.countDelta must be an integer")
        quarantine = count(
            expected["quarantineCount"], f"{row_path}.expected.quarantineCount"
        )
        for key in ("preserveStableIds", "preserveContent"):
            if not isinstance(expected[key], bool):
                fail(f"{row_path}.expected.{key} must be boolean")
        if post_count - pre_count != delta:
            fail(f"{row_path} count delta does not match its expectation")
        if quarantine > post_count:
            fail(f"{row_path} quarantine count exceeds the post-migration count")
        if expected["preserveStableIds"] and pre_ids != post_ids:
            fail(f"{row_path} changed stable IDs")
        if expected["preserveContent"] and pre_content != post_content:
            fail(f"{row_path} changed durable content")
        if quarantine and name not in {"jobs", "artifacts", "artifact_candidates"}:
            fail(f"{row_path} is not an approved quarantine-bearing inventory")


def validate(document: Any) -> None:
    if not isinstance(document, dict):
        fail("rehearsal manifest must be an object")
    if document.get("schemaVersion") != "labby.multi-user-migration-rehearsal/v1":
        fail("unsupported rehearsal manifest schemaVersion")
    checkpoint = digest(document.get("checkpointSha256"), "checkpointSha256")
    if digest(document.get("rollbackCheckpointSha256"), "rollbackCheckpointSha256") != checkpoint:
        fail("rollback checkpoint must exactly match the pre-migration checkpoint")
    systems = document.get("systems")
    if not isinstance(systems, dict) or set(systems) != set(REQUIRED_INVENTORIES):
        fail("systems must contain exactly labby and depot")
    for system in REQUIRED_INVENTORIES:
        value = systems[system]
        if not isinstance(value, dict) or set(value) != {"inventory"}:
            fail(f"systems.{system} must contain inventory exactly")
        validate_inventory(system, value["inventory"])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    try:
        validate(json.loads(args.manifest.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError, ValueError) as error:
        raise SystemExit(f"migration rehearsal rejected: {error}") from error


if __name__ == "__main__":
    main()
