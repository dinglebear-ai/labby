#!/usr/bin/env python3
"""Syntax-check delivery goldens; Rust conformance owns semantic validation."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path


ROOT = Path(__file__).parent
FIXTURES = ROOT / "fixtures"
DELIVERY_SCHEMA = "dinglebear.depot-delivery/v1"
MANIFEST_SCHEMA = "dinglebear.depot-delivery-manifest/v1"
DNS_VECTOR_SCHEMA = "dinglebear.depot-dns-policy-v1/vectors"
SECRET_KEYS = {"grant", "token", "authorization", "credential", "secret", "password", "cookie"}
EXPECTED = {
    "chunk-manifest.json",
    "delivery-error-expired-grant.json",
    "delivery-error-replayed-grant.json",
    "delivery-error-revoked-grant.json",
    "delivery-error-wrong-target.json",
    "delivery-receipt-activated.json",
    "delivery-receipt-stored-activation-failed.json",
    "delivery-request.json",
    "dns-policy-vectors.json",
    "download-grant-claims.json",
    "identity-link-challenge.json",
    "identity-link-receipt.json",
}


def load(path: Path) -> dict:
    def no_duplicates(pairs: list[tuple[str, object]]) -> dict:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"{path.name}: duplicate key {key!r}")
            result[key] = value
        return result

    with path.open(encoding="utf-8") as stream:
        value = json.load(stream, object_pairs_hook=no_duplicates)
    if not isinstance(value, dict):
        raise ValueError(f"{path.name}: fixture root must be an object")
    return value


def inspect_secrets(value: object, where: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in SECRET_KEYS:
                raise ValueError(f"{where}: secret-shaped field {key!r}")
            inspect_secrets(child, f"{where}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            inspect_secrets(child, f"{where}[{index}]")


def validate_dns_vectors(value: dict) -> None:
    if set(value) != {"schemaVersion", "vectors"} or value["schemaVersion"] != DNS_VECTOR_SCHEMA:
        raise ValueError("dns-policy-vectors.json: invalid closed vector envelope")
    vectors = value["vectors"]
    if not isinstance(vectors, list) or len(vectors) != 3:
        raise ValueError("dns-policy-vectors.json: expected three vectors")
    names: set[str] = set()
    for vector in vectors:
        if not isinstance(vector, dict) or set(vector) != {
            "name", "origin", "addresses", "preimage", "dnsPolicyId"
        }:
            raise ValueError("dns-policy-vectors.json: invalid closed vector shape")
        names.add(vector["name"])
        expected = "dns_" + hashlib.sha256(vector["preimage"].encode("utf-8")).hexdigest()
        if vector["dnsPolicyId"] != expected:
            raise ValueError(f"dns vector {vector['name']}: digest mismatch")
    if names != {"ipv4", "ipv6", "mixed"}:
        raise ValueError("dns-policy-vectors.json: missing family coverage")


def main() -> int:
    paths = sorted(FIXTURES.glob("*.json"))
    names = {path.name for path in paths}
    if names != EXPECTED:
        raise ValueError(f"fixture inventory mismatch: missing={sorted(EXPECTED - names)} extra={sorted(names - EXPECTED)}")
    for path in paths:
        value = load(path)
        inspect_secrets(value)
        if path.name == "dns-policy-vectors.json":
            validate_dns_vectors(value)
        elif path.name == "chunk-manifest.json":
            if value.get("schemaVersion") != MANIFEST_SCHEMA:
                raise ValueError(f"{path.name}: schema version mismatch")
        elif path.name == "download-grant-claims.json":
            if value.get("protocolVersion") != DELIVERY_SCHEMA:
                raise ValueError(f"{path.name}: protocol version mismatch")
        elif value.get("schemaVersion") != DELIVERY_SCHEMA:
            raise ValueError(f"{path.name}: schema version mismatch")
        print(f"ok {path.relative_to(ROOT)}")
    print(f"syntax-validated {len(paths)} fixtures; Rust conformance is the semantic authority")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"validation failed: {error}", file=sys.stderr)
        sys.exit(1)
