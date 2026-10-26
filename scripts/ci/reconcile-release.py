#!/usr/bin/env python3
"""Fail closed unless all remote release subjects and distributions agree."""
from __future__ import annotations
import argparse, json
from pathlib import Path
try:
    from release_channel import identity
except ModuleNotFoundError:
    from scripts.ci.release_channel import identity

REQUIRED_SURFACES = ("github", "npm", "mcp")
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
for row in expected.get("auxiliary", []):
    want[row["name"]] = row["sha256"]
got = {row["name"]: row["sha256"] for row in observed.get("subjects", [])}
missing = sorted(want.keys() - got.keys())
unexpected = sorted((got.keys() - want.keys()) | set(observed.get("unexpected_assets", [])))
mismatched = sorted(name for name in want.keys() & got.keys() if want[name] != got[name])
expected_dist = expected.get("distributions", {})
observed_dist = observed.get("distributions", {})
distribution_errors = {}
release = identity(expected["tag"])
if "package" in expected_dist.get("npm", {}) and expected_dist["npm"].get("tag", "latest") != release["npm_tag"]:
    distribution_errors["npm_channel"] = "manifest npm pointer does not match release channel"
if "package" in expected_dist.get("npm", {}) and expected_dist["npm"].get("version") != release["version"]:
    distribution_errors["npm_version"] = "manifest npm version does not match release tag"
if release["channel"] == "dev" and expected_dist.get("github", {}).get("prerelease") is not True:
    distribution_errors["github_channel"] = "dev manifest must require a prerelease"

required = REQUIRED_SURFACES if identity(expected["tag"])["channel"] == "stable" else ("github", "npm")
if identity(expected["tag"])["channel"] == "dev" and "mcp" in expected_dist:
    distribution_errors["mcp"] = "dev releases must not publish the registry distribution"
for surface in required:
    if surface not in expected_dist:
        distribution_errors[surface] = "missing expectation"
    elif surface not in observed_dist:
        distribution_errors[surface] = "not observed"
    elif observed_dist[surface] != expected_dist[surface]:
        distribution_errors[surface] = {"expected": expected_dist[surface], "observed": observed_dist[surface]}
if "incus" in expected_dist:
    if "incus" not in observed_dist:
        distribution_errors["incus"] = "not observed"
    elif observed_dist["incus"] != expected_dist["incus"]:
        distribution_errors["incus"] = {"expected": expected_dist["incus"], "observed": observed_dist["incus"]}
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
