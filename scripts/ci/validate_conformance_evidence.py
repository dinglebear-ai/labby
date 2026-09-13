#!/usr/bin/env python3
"""Validate required lifecycle conformance evidence against captured provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import stat
from pathlib import Path
from typing import Any


REQUIRED_CASES = {
    "success": "real_process",
    "disconnect": "real_process",
    "tool-error": "real_process",
    "timeout": "real_process",
    "document-invalidation": "real_process",
    "generation-replacement": "real_process",
    "admit-cancel-late-cleanup": "real_process",
    "dispatch-cancel-late-cleanup": "real_process",
    "divergent-adapter-self-test": "negative_adapter_self_test",
}
MAX_REPORT_BYTES = 1_048_576
MAX_PROVENANCE_BYTES = 4_096
REQUIRED_ACTIONS = {
    "success": ["admit", "dispatch", "complete_success"],
    "disconnect": ["admit", "dispatch", "disconnect"],
    "tool-error": ["admit", "dispatch", "complete_error"],
    "timeout": ["admit", "dispatch", "timeout"],
    "document-invalidation": ["admit", "dispatch", "invalidate_document"],
    "generation-replacement": ["admit", "dispatch", "replace_connection", "admit", "dispatch", "complete_success"],
    "admit-cancel-late-cleanup": ["admit", "cancel", "complete_success"],
    "dispatch-cancel-late-cleanup": ["admit", "dispatch", "cancel", "complete_success"],
    "divergent-adapter-self-test": ["admit", "dispatch", "complete_success"],
}


def fail(message: str) -> None:
    raise ValueError(message)


def canonical_trace_fingerprint(steps: list[Any]) -> str:
    encoded = json.dumps(
        steps,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode()
    return hashlib.sha256(encoded).hexdigest()


def read_bounded(path: Path, limit: int) -> str:
    try:
        metadata = path.stat()
        if not stat.S_ISREG(metadata.st_mode):
            fail(f"evidence file must be regular: {path}")
        if metadata.st_size > limit:
            fail(f"evidence file exceeds {limit} bytes: {path}")
        with path.open("rb") as stream:
            contents = stream.read(limit + 1)
        if len(contents) > limit:
            fail(f"evidence file exceeds {limit} bytes: {path}")
        return contents.decode("utf-8")
    except (OSError, UnicodeDecodeError) as error:
        fail(f"cannot read evidence file {path}: {error}")


def is_lower_hex(value: Any, length: int) -> bool:
    return (
        isinstance(value, str)
        and len(value) == length
        and all(character in "0123456789abcdef" for character in value)
    )


def captured_provenance(evidence_dir: Path) -> dict[str, Any]:
    git_sha = read_bounded(
        evidence_dir / "source-revision.txt", MAX_PROVENANCE_BYTES
    ).strip()
    dirty_text = read_bounded(
        evidence_dir / "source-dirty.txt", MAX_PROVENANCE_BYTES
    )
    binary_line = read_bounded(
        evidence_dir / "binary-sha256.txt", MAX_PROVENANCE_BYTES
    ).strip()
    binary_parts = binary_line.split()
    if not is_lower_hex(git_sha, 40):
        fail("captured source revision is not a 40-character lowercase hex commit")
    if not binary_parts or not is_lower_hex(binary_parts[0], 64):
        fail("captured binary SHA-256 is invalid")
    return {
        "git_sha": git_sha,
        "git_dirty": bool(dirty_text.strip()),
        "binary_sha256": binary_parts[0],
    }


def validate_record(
    path: Path,
    record: dict[str, Any],
    expected_kind: str,
    provenance: dict[str, Any],
) -> None:
    case_id = record.get("case_id")
    if type(record.get("schema_version")) is not int or record.get("schema_version") != 1 or record.get("lane") != "conformance":
        fail(f"invalid conformance evidence envelope: {path}")
    if record.get("evidence_kind") != expected_kind:
        fail(f"wrong evidence kind for {case_id}")
    if record.get("relation") != "browser-public-v1":
        fail(f"wrong observation relation for {case_id}")

    steps = record.get("steps")
    if not isinstance(steps, list) or not steps:
        fail(f"missing controlled steps for {case_id}")
    if any(not isinstance(step, dict) for step in steps) or [step.get("action") for step in steps] != REQUIRED_ACTIONS[case_id]:
        fail(f"controlled action sequence differs for {case_id}")
    expected_fingerprint = canonical_trace_fingerprint(steps)
    if record.get("trace_fingerprint") != expected_fingerprint:
        fail(f"trace fingerprint does not match canonical steps for {case_id}")

    observations = record.get("observations")
    if not isinstance(observations, list):
        fail(f"missing observations for {case_id}")
    if not all(isinstance(row, dict) for row in observations):
        fail(f"observations must be JSON objects for {case_id}")
    positions = [row.get("position") for row in observations]
    expected_positions = list(range(len(steps) + 1))
    if any(type(position) is not int for position in positions) or positions != expected_positions:
        fail(
            f"observations must cover every position exactly once for {case_id}: "
            f"expected {expected_positions}, got {positions}"
        )
    if expected_kind == "negative_adapter_self_test":
        if len(steps) != 3 or record.get("failure") != {"kind": "observation_divergence", "position": 3}:
            fail(f"negative adapter did not retain the expected divergence for {case_id}")
        if any(row.get("relation_passed") is not (row["position"] != 3) for row in observations):
            fail(f"negative adapter observations do not match the expected divergence for {case_id}")
    elif record.get("failure") is not None or any(row.get("relation_passed") is not True for row in observations):
        fail(f"observation relation did not pass at every position for {case_id}")

    cleanup = record.get("cleanup")
    if not isinstance(cleanup, dict):
        fail(f"cleanup evidence must be an object for {case_id}")
    if cleanup.get("clean") is not True or cleanup.get("failures") != []:
        fail(f"cleanup did not complete for {case_id}")
    if record.get("verdict") != "passed":
        fail(f"required case did not pass: {case_id}")

    if expected_kind in {"real_process", "negative_adapter_self_test"}:
        source = record.get("source")
        if not isinstance(source, dict):
            fail(f"real-process evidence lacks source identity: {case_id}")
        if type(source.get("git_dirty")) is not bool:
            fail(f"real-process source.git_dirty must be boolean: {case_id}")
        for field, expected in provenance.items():
            if source.get(field) != expected:
                fail(f"real-process source.{field} does not match captured provenance: {case_id}")
        fixtures = source.get("fixture_versions")
        if (
            not isinstance(fixtures, list)
            or not fixtures
            or not all(isinstance(fixture, str) and fixture for fixture in fixtures)
        ):
            fail(f"real-process evidence lacks fixture versions: {case_id}")


def validate(evidence_dir: Path) -> None:
    provenance = captured_provenance(evidence_dir)
    for case_id, expected_kind in REQUIRED_CASES.items():
        path = evidence_dir / f"{case_id}.json"
        try:
            record = json.loads(read_bounded(path, MAX_REPORT_BYTES))
        except json.JSONDecodeError as error:
            fail(f"cannot read conformance JSON {path}: {error}")
        if not isinstance(record, dict):
            fail(f"conformance evidence must be a JSON object: {path}")
        if record.get("case_id") != case_id:
            fail(f"required case file has wrong case_id: {path}")
        validate_record(path, record, expected_kind, provenance)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        validate(args.evidence_dir)
    except ValueError as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
