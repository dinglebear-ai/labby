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
REQUIRED_FLOWS = {
    "bootstrap", "bazaarBrowse", "artifactDetail", "sendToLabby",
    "candidateIntake", "fork", "publication", "license", "archive",
    "delete", "browserCredentialAdministration", "maintenance",
}
DISABLED_MOUNTS = {"staticBearerBrowser", "webUiAuthDisabled", "noAuth", "syntheticDevelopment"}


def fail(message: str) -> None:
    raise ValueError(message)


def validate(data: dict) -> None:
    if data.get("schemaVersion") != "labby.depot-compatibility/v1":
        fail("unsupported schemaVersion")
    if not REQUIRED_ROUTES.issubset(set(data["depot"]["routes"])):
        fail("required route missing")
    if not REQUIRED_FLOWS.issubset(data["flows"]):
        fail("required flow missing")
    if any(data["flows"][name]["status"] != "supported" for name in REQUIRED_FLOWS):
        fail("operational flow is not supported")
    if not data["flows"]["sendToLabby"].get("exactExport"):
        fail("exact import requires the exact export contract")
    if any(data["mountPolicy"].get(name) != "disabled" for name in DISABLED_MOUNTS):
        fail("unsafe browser auth mode is enabled")
    if data["actorPolicy"].get("serviceCredential") != "mutations-require-labby-admin-csrf-and-depot-write-scope":
        fail("service credential mutation boundary is not explicit")
    administration = data.get("administrationContract", {})
    if administration.get("version") != 1:
        fail("administration contract version missing")
    fingerprint = administration.get("operationFingerprint", {})
    if fingerprint.get("algorithm") != "sha256-canonical-json" or fingerprint.get("required") is not True:
        fail("operation fingerprint policy missing")
    schema = administration.get("inputSchema", {})
    if schema.get("dialect") != "labby.depot-operation-schema/v1":
        fail("operation schema dialect missing")
    if not {"string", "boolean", "integer", "number", "array", "object"}.issubset(set(schema.get("types", []))):
        fail("operation schema type contract incomplete")
    if not (1 <= schema.get("maxProperties", 0) <= 128):
        fail("operation property limit is unbounded")
    if not (1 <= schema.get("maxEnumValues", 0) <= 256):
        fail("operation enum limit is unbounded")
    limits = data["limits"]
    if not (1 <= limits["artifactPage"] <= 200):
        fail("artifact page limit is unbounded")
    if limits["streamConcurrency"] > limits["interactiveConcurrency"]:
        fail("stream concurrency can starve interactive calls")


def main() -> None:
    try:
        validate(json.loads(MANIFEST.read_text()))
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"depot control-plane contract: {error}") from error
    print(MANIFEST.relative_to(ROOT))


if __name__ == "__main__":
    main()
