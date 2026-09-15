#!/usr/bin/env python3
"""Qualify actual daemon lifecycle captures through bounded incident exploration."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

if __package__:
    from .validate_conformance_evidence import (
        MAX_REPORT_BYTES, REQUIRED_ACTIONS, read_bounded, validate,
    )
else:
    from validate_conformance_evidence import (
        MAX_REPORT_BYTES, REQUIRED_ACTIONS, read_bounded, validate,
    )


def qualify_result(case: str, source: dict, result: dict) -> None:
    """Assert the exact observed prefix, not merely an exit code or a verdict."""
    reduced = result["incident"]
    scenario = reduced["scenario"]
    replay = reduced["replay"]
    actions = ["connect", *REQUIRED_ACTIONS[case]]
    if case in {"admit-cancel-late-cleanup", "dispatch-cancel-late-cleanup"}:
        # Unmatched late completions are acknowledgements, not state transitions,
        # and the authoritative emitter deliberately does not log them.
        actions.pop()
    if [step["action"] for step in scenario["steps"]] != actions:
        raise ValueError(f"incident prefix does not match actual case: {case}")
    if (scenario["project"] != "labby" or scenario["model"] != "browser_request"
            or scenario["invariant"] != "LABBY-REQ-005"
            or scenario["origin"]["kind"] != "incident"
            or scenario["status"] != "unreproduced"
            or scenario["expect"] != "invariant_violated"
            or replay["verdict"] != "invariant_holds"
            or replay["gate_failure"] is not False
            or replay["matches_expectation"] is not False
            or replay["fingerprint"] != scenario["fingerprint"]):
        raise ValueError(f"incident qualification overclaims reproduction: {case}")
    observations = replay["observations"]
    if (any(type(item["position"]) is not int for item in observations)
            or [item["position"] for item in observations] != list(range(len(actions) + 1))):
        raise ValueError(f"incident replay is incomplete: {case}")
    if any(item["outcome"] != {"outcome": "applied"} for item in observations[1:]):
        raise ValueError(f"incident invented rejected transitions: {case}")
    exploration = result["exploration"]
    if (exploration["backend"] != "stateright"
            or exploration["invariant"] != scenario["invariant"]
            or exploration["verdict"]["verdict"] != "bounded"
            or not exploration["verdict"].get("bounds") or exploration["scenarios"]):
        raise ValueError(f"incident neighborhood did not qualify: {case}")
    encoded = json.dumps(reduced)
    for event in source["events"]:
        fields = event["fields"]
        for key in ("generation_id", "previous_generation_id", "call_id"):
            identifier = fields.get(key)
            if identifier and identifier in encoded:
                raise ValueError(f"incident retains a raw runtime identifier: {case}")


def run(evidence_dir: Path, verifier: Path) -> None:
    validate(evidence_dir)
    verifier = verifier.resolve(strict=True)
    destination = evidence_dir / "incidents"
    destination.mkdir(exist_ok=True)
    # The product identity remains in each C1 source record. The incident host
    # is a distinct executable and gets its own independently measured digest.
    with verifier.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    (destination / "verifier-sha256.txt").write_text(digest + "\n")
    for case in REQUIRED_ACTIONS:
        record = json.loads(read_bounded(evidence_dir / f"{case}.json", MAX_REPORT_BYTES))
        source = record.get("incident_input")
        if not isinstance(source, dict):
            raise ValueError(f"missing actual daemon lifecycle capture: {case}")
        with tempfile.TemporaryDirectory(prefix="labby-incident-") as temporary:
            input_path = Path(temporary) / "input.json"
            input_path.write_text(json.dumps(source))
            output_path = destination / f"{case}.json"
            # Redirect bounded host output to files, not unbounded PIPE buffers.
            with output_path.open("wb") as output, tempfile.TemporaryFile() as errors:
                result = subprocess.run(
                    [str(verifier), "incident-explore", str(input_path), "LABBY-REQ-005"],
                    stdout=output, stderr=errors, timeout=20, check=False,
                )
            if result.returncode != 0:
                raise ValueError(f"incident host failed for {case}; no qualification claimed")
            qualified = json.loads(read_bounded(output_path, MAX_REPORT_BYTES))
            qualify_result(case, source, qualified)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        run(arguments.evidence_dir, arguments.verifier)
    except (ValueError, OSError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f"incident evidence failed: {error}\n")
    print("All real daemon incident prefixes replayed and explored within bounds.")
