#!/usr/bin/env python3
"""Validate the checked Labby/Depot compatibility denominator."""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "docs/contracts/fixtures/depot-control-plane/compatibility-v1.json"

REQUIRED_ROUTES = {
    "GET /api/session",
    "GET /api/operations",
    "POST /api/operations/:operation",
    "POST /api/artifacts/exact",
    "GET /api/artifacts/components/:artifact_id/:revision_id/:component_id",
}
REQUIRED_FLOWS = {"bootstrap", "bazaarBrowse", "artifactDetail", "sendToLabby"}
DISABLED_MOUNTS = {"staticBearerBrowser", "webUiAuthDisabled", "noAuth", "syntheticDevelopment"}


def fail(message: str) -> None:
    raise SystemExit(f"depot control-plane contract: {message}")


def main() -> None:
    data = json.loads(MANIFEST.read_text())
    if data.get("schemaVersion") != "labby.depot-compatibility/v1":
        fail("unsupported schemaVersion")
    if not REQUIRED_ROUTES.issubset(set(data["depot"]["routes"])):
        fail("required route missing")
    if not REQUIRED_FLOWS.issubset(data["flows"]):
        fail("required flow missing")
    if any(data["flows"][name]["status"] != "supported" for name in REQUIRED_FLOWS):
        fail("operational flow is not supported")
    if any(data["mountPolicy"].get(name) != "disabled" for name in DISABLED_MOUNTS):
        fail("unsafe browser auth mode is enabled")
    if data["actorPolicy"].get("serviceCredential") != "read-only-unless-explicitly-approved":
        fail("service credential policy broadened")
    limits = data["limits"]
    if not (1 <= limits["artifactPage"] <= 200):
        fail("artifact page limit is unbounded")
    if limits["streamConcurrency"] > limits["interactiveConcurrency"]:
        fail("stream concurrency can starve interactive calls")
    print(MANIFEST.relative_to(ROOT))


if __name__ == "__main__":
    main()
