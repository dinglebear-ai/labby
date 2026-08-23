#!/usr/bin/env python3
"""Validate Depot delivery v1 golden fixtures with only the Python stdlib."""

from __future__ import annotations

import json
import hashlib
import re
import sys
from pathlib import Path


ROOT = Path(__file__).parent
FIXTURES = ROOT / "fixtures"
SCHEMA = "dinglebear.depot-delivery/v1"
DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
SECRET_KEYS = {"grant", "token", "authorization", "credential", "secret"}
STATES = {
    "requested", "granted", "transferred", "verified", "stored",
    "materialized", "exposed", "activated", "incompatible", "partial",
    "cancelled", "failed",
}


def load(path: Path) -> dict:
    def no_duplicates(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate key {key!r}")
            result[key] = value
        return result

    with path.open(encoding="utf-8") as stream:
        value = json.load(stream, object_pairs_hook=no_duplicates)
    if not isinstance(value, dict):
        raise ValueError("fixture root must be an object")
    return value


def require_keys(value: dict, keys: set[str], where: str) -> None:
    missing = keys - value.keys()
    if missing:
        raise ValueError(f"{where}: missing {sorted(missing)}")


def inspect_secrets(value: object, where: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in SECRET_KEYS:
                raise ValueError(f"{where}: secret-shaped field {key!r}")
            inspect_secrets(child, f"{where}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            inspect_secrets(child, f"{where}[{index}]")


def validate_resource(resource: dict, where: str) -> None:
    require_keys(resource, {"kind", "id", "revisionId", "contentDigest"}, where)
    if resource["kind"] not in {"artifact", "loadout"}:
        raise ValueError(f"{where}: invalid kind")
    if not DIGEST.fullmatch(resource["contentDigest"]):
        raise ValueError(f"{where}: invalid content digest")


def validate_request(value: dict) -> None:
    require_keys(value, {
        "schemaVersion", "deliveryHandle", "connectionId", "targetId",
        "resource", "conflictPolicy", "requestedOperations",
        "idempotencyKey", "correlationId",
    }, "request")
    if value["conflictPolicy"] not in {"reject", "keep_existing", "create_side_by_side"}:
        raise ValueError("request: invalid conflict policy")
    if not value["requestedOperations"]:
        raise ValueError("request: operations must not be empty")
    validate_resource(value["resource"], "request.resource")


def validate_claims(value: dict) -> None:
    require_keys(value, {
        "iss", "sub", "aud", "tenantId", "targetId", "connectionId",
        "deliveryId", "resourceKind", "resourceId", "revisionId",
        "contentDigest", "manifestDigest", "purpose", "protocolVersion",
        "artifactSchemaVersion", "jti", "iat", "nbf", "exp",
    }, "claims")
    if value["aud"] != "labby:delivery" or value["purpose"] != "depot-to-labby-pull":
        raise ValueError("claims: invalid audience or purpose")
    if value["protocolVersion"] != SCHEMA:
        raise ValueError("claims: protocol version mismatch")
    if value["artifactSchemaVersion"] != "dinglebear.artifact-interchange/v1":
        raise ValueError("claims: Artifact schema mismatch")
    if value["nbf"] < value["iat"] or value["exp"] - value["iat"] > 300:
        raise ValueError("claims: invalid validity window")
    if not DIGEST.fullmatch(value["contentDigest"]) or not DIGEST.fullmatch(value["manifestDigest"]):
        raise ValueError("claims: invalid digest")


def validate_receipt(value: dict) -> None:
    require_keys(value, {
        "schemaVersion", "receiptId", "sequence", "deliveryId",
        "correlationId", "connectionId", "tenantId", "targetId",
        "resource", "state", "components", "summary", "occurredAt",
    }, "receipt")
    validate_resource(value["resource"], "receipt.resource")
    if value["state"] not in STATES or value["sequence"] < 1:
        raise ValueError("receipt: invalid state or sequence")
    if not value["components"]:
        raise ValueError("receipt: components must not be empty")
    for component in value["components"]:
        require_keys(component, {"componentId", "state"}, "receipt.component")
        if component["state"] not in STATES:
            raise ValueError("receipt.component: invalid state")
    require_keys(value["summary"], STATES, "receipt.summary")
    if any(not isinstance(count, int) or count < 0 for count in value["summary"].values()):
        raise ValueError("receipt.summary: counts must be nonnegative integers")


def validate_error(value: dict) -> None:
    require_keys(value, {"schemaVersion", "error", "deliveryId", "correlationId", "targetId"}, "error fixture")
    require_keys(value["error"], {"code", "stage", "retryable", "message"}, "error")
    if not isinstance(value["error"]["retryable"], bool):
        raise ValueError("error.retryable must be boolean")


def validate_link_challenge(value: dict) -> None:
    require_keys(value, {
        "schemaVersion", "challengeId", "nonce", "targetId",
        "targetDisplayName", "depotOrigin", "labbyKeyThumbprint",
        "protocolRange", "expiresAt",
    }, "link challenge")
    if not value["depotOrigin"].startswith("https://"):
        raise ValueError("link challenge: Depot origin must use HTTPS")
    if len(value["nonce"]) < 43 or not DIGEST.fullmatch(value["labbyKeyThumbprint"]):
        raise ValueError("link challenge: weak nonce or invalid key thumbprint")
    if set(value["protocolRange"].values()) != {SCHEMA}:
        raise ValueError("link challenge: unsupported protocol range")


def validate_link_receipt(value: dict) -> None:
    require_keys(value, {
        "schemaVersion", "challengeId", "connectionId", "depotAccountId",
        "tenantId", "targetId", "depotOrigin", "depotKeyThumbprint",
        "labbyKeyThumbprint", "protocolVersion", "linkedAt",
    }, "link receipt")
    if value["protocolVersion"] != SCHEMA:
        raise ValueError("link receipt: protocol version mismatch")
    for key in ("depotKeyThumbprint", "labbyKeyThumbprint"):
        if not DIGEST.fullmatch(value[key]):
            raise ValueError(f"link receipt: invalid {key}")


def validate_manifest(value: dict) -> None:
    require_keys(value, {
        "schemaVersion", "deliveryId", "targetId", "revisionId",
        "contentDigest", "totalCompressedBytes", "totalUncompressedBytes",
        "components", "chunks",
    }, "manifest")
    if value["schemaVersion"] != "dinglebear.depot-delivery-manifest/v1":
        raise ValueError("manifest: schema version mismatch")
    if len(value["components"]) > 2_000 or len(value["chunks"]) > 4_096:
        raise ValueError("manifest: count limit exceeded")
    if value["totalCompressedBytes"] > 1 << 30 or value["totalUncompressedBytes"] > 2 << 30:
        raise ValueError("manifest: byte limit exceeded")
    if sum(chunk["bytes"] for chunk in value["chunks"]) != value["totalCompressedBytes"]:
        raise ValueError("manifest: compressed byte total mismatch")
    ordinals = [chunk["ordinal"] for chunk in value["chunks"]]
    if ordinals != list(range(len(ordinals))):
        raise ValueError("manifest: chunk ordinals must be contiguous")
    for chunk in value["chunks"]:
        if chunk["bytes"] > 8 << 20 or not DIGEST.fullmatch(chunk["digest"]):
            raise ValueError("manifest: invalid chunk")
        if not chunk["downloadPath"].startswith("/") or "://" in chunk["downloadPath"]:
            raise ValueError("manifest: download path must be origin-relative")


def main() -> int:
    paths = sorted(FIXTURES.glob("*.json"))
    if not paths:
        raise ValueError("no JSON fixtures found")
    identities = {}
    loaded = {}
    for path in paths:
        value = load(path)
        loaded[path.name] = value
        if path.name not in {"download-grant-claims.json", "chunk-manifest.json"} and value.get("schemaVersion") != SCHEMA:
            raise ValueError(f"{path.name}: schema version mismatch")
        if path.name == "delivery-request.json":
            validate_request(value)
        elif path.name == "download-grant-claims.json":
            validate_claims(value)
        elif path.name == "chunk-manifest.json":
            validate_manifest(value)
        elif path.name == "identity-link-challenge.json":
            validate_link_challenge(value)
        elif path.name == "identity-link-receipt.json":
            validate_link_receipt(value)
        elif path.name.startswith("delivery-receipt-"):
            validate_receipt(value)
        elif path.name.startswith("delivery-error-"):
            validate_error(value)
        else:
            raise ValueError(f"unrecognized fixture {path.name}")
        inspect_secrets(value)
        for key in ("deliveryId", "correlationId", "targetId"):
            if key in value:
                previous = identities.setdefault(key, value[key])
                if value[key] != previous:
                    raise ValueError(f"{path.name}: cross-fixture {key} mismatch")
        print(f"ok {path.relative_to(ROOT)}")
    manifest_bytes = json.dumps(
        loaded["chunk-manifest.json"], sort_keys=True, separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    manifest_digest = "sha256:" + hashlib.sha256(manifest_bytes).hexdigest()
    if loaded["download-grant-claims.json"]["manifestDigest"] != manifest_digest:
        raise ValueError("grant claims: manifest digest does not bind the golden manifest")
    print(f"validated {len(paths)} fixtures")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"validation failed: {error}", file=sys.stderr)
        sys.exit(1)
