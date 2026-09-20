#!/usr/bin/env python3
"""Fail closed when reviewed Incus supply inputs drift from consumers."""

import argparse
import hashlib
import json
import pathlib
import re
import sys


def fail(message: str) -> None:
    raise SystemExit(f"Incus supply validation failed: {message}")


parser = argparse.ArgumentParser()
parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).parents[2])
parser.add_argument("--emit-identity", action="store_true")
args = parser.parse_args()
root = args.root.resolve()

incus_path = root / "config/incus/provision-supply.json"
incus_image = (root / "config/incus/labby-image.yaml").read_text()

incus = json.loads(incus_path.read_text())
if not isinstance(incus, dict) or not incus:
    fail("empty Incus supply manifest")
supply_chunks: dict[str, str] = {}
for chunk in incus_image.split("# LABBY_SUPPLY: ")[1:]:
    name = chunk.splitlines()[0].strip()
    if name in supply_chunks:
        fail(f"duplicate Incus supply consumer {name}")
    supply_chunks[name] = chunk.split("# LABBY_SUPPLY: ", 1)[0]
if set(incus) != set(supply_chunks):
    fail(f"Incus supply manifest/consumer names differ: {sorted(set(incus) ^ set(supply_chunks))}")
for name, item in incus.items():
    if not isinstance(item, dict) or not isinstance(item.get("version"), str):
        fail(f"invalid Incus supply entry {name}")
    proof = item.get("sha256") or item.get("integrity")
    if "sha256" in item and not re.fullmatch(r"[0-9a-f]{64}", item["sha256"]):
        fail(f"invalid sha256 for Incus supply entry {name}")
    if "integrity" in item and not re.fullmatch(r"sha512-[A-Za-z0-9+/]+={0,2}", item["integrity"]):
        fail(f"invalid integrity for Incus supply entry {name}")
    section = supply_chunks[name]
    if item["version"] not in section or (proof and proof not in section):
        fail(f"Incus image does not consume exact manifest entry {name}")

canonical = json.dumps(
    {"incus": incus},
    sort_keys=True,
    separators=(",", ":"),
).encode()
identity = hashlib.sha256(canonical).hexdigest()
if args.emit_identity:
    print(identity)
