#!/usr/bin/env python3
"""Fail closed unless all remote release subjects and distributions agree."""
from __future__ import annotations
import argparse, json
from pathlib import Path

SURFACES = ("github", "npm", "ghcr", "incus", "mcp")
parser = argparse.ArgumentParser()
parser.add_argument("--manifest", type=Path, required=True)
parser.add_argument("--observed", type=Path, required=True)
args = parser.parse_args()
expected = json.loads(args.manifest.read_text())
observed = json.loads(args.observed.read_text())
if expected.get("schema") != "ai.dinglebear.labby/release-manifest/v1":
    raise SystemExit("unsupported release manifest schema")
want = {row["name"]: row["sha256"] for row in expected["subjects"]}
for row in expected["subjects"]:
    want[row["sbom"]["name"]] = row["sbom"]["sha256"]
image_sbom = expected.get("distributions", {}).get("ghcr", {}).get("sbom")
if image_sbom:
    want[image_sbom["name"]] = image_sbom["sha256"]
for row in expected.get("auxiliary", []):
    want[row["name"]] = row["sha256"]
got = {row["name"]: row["sha256"] for row in observed.get("subjects", [])}
missing = sorted(want.keys() - got.keys())
unexpected = sorted((got.keys() - want.keys()) | set(observed.get("unexpected_assets", [])))
mismatched = sorted(name for name in want.keys() & got.keys() if want[name] != got[name])
expected_dist = expected.get("distributions", {})
observed_dist = observed.get("distributions", {})
distribution_errors = {}
for surface in SURFACES:
    if surface not in expected_dist:
        distribution_errors[surface] = "missing expectation"
    elif surface not in observed_dist:
        distribution_errors[surface] = "not observed"
    elif observed_dist[surface] != expected_dist[surface]:
        distribution_errors[surface] = {"expected": expected_dist[surface], "observed": observed_dist[surface]}
observed_attestations = {row.get("subject"): row.get("status") for row in observed.get("attestations", [])}
attestation_errors = {
    row["subject"]: observed_attestations.get(row["subject"], "not observed")
    for row in expected.get("attestations", [])
    if observed_attestations.get(row["subject"]) != "verified"
}
complete = not (missing or unexpected or mismatched or distribution_errors or attestation_errors)
report = {"tag": expected["tag"], "complete": complete, "missing": missing, "unexpected": unexpected, "mismatched": mismatched, "distribution_errors": distribution_errors, "attestation_errors": attestation_errors}
print(json.dumps(report, sort_keys=True))
raise SystemExit(0 if complete else 1)
