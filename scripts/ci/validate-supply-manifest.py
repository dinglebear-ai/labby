#!/usr/bin/env python3
"""Fail closed when reviewed container supply inputs drift from consumers."""

import argparse
import hashlib
import json
import pathlib
import re
import sys


def fail(message: str) -> None:
    raise SystemExit(f"container supply validation failed: {message}")


parser = argparse.ArgumentParser()
parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).parents[2])
parser.add_argument("--emit-identity", action="store_true")
args = parser.parse_args()
root = args.root.resolve()

conf_path = root / "config/container-supply.conf"
dockerfile = (root / "config/Dockerfile").read_text()
dev_dockerfile = (root / "config/Dockerfile.fast").read_text()
incus_path = root / "config/incus/provision-supply.json"
incus_image = (root / "config/incus/labby-image.yaml").read_text()

conf: dict[str, str] = {}
for number, raw in enumerate(conf_path.read_text().splitlines(), 1):
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    if "=" not in line:
        fail(f"{conf_path}:{number}: expected KEY=VALUE")
    key, value = line.split("=", 1)
    if not re.fullmatch(r"LABBY_[A-Z0-9_]+", key) or not value or any(c.isspace() for c in value):
        fail(f"{conf_path}:{number}: invalid supply assignment")
    if key in conf:
        fail(f"duplicate supply key {key}")
    conf[key] = value
    if f"${{{key}}}" not in dockerfile:
        fail(f"Dockerfile does not consume {key}")

if not conf:
    fail("empty Docker supply manifest")
declared_args = set(re.findall(r"(?m)^ARG (LABBY_[A-Z0-9_]+)(?:=.*)?$", dockerfile))
consumer_keys = set(re.findall(r"\$\{(LABBY_[A-Z0-9_]+)\}", dockerfile)) & declared_args
expected_consumers = conf.keys()
if consumer_keys != set(expected_consumers) or declared_args != set(expected_consumers):
    fail(
        "Docker supply manifest/consumer keys differ: "
        f"unconsumed={sorted(set(expected_consumers) - consumer_keys)} "
        f"undeclared={sorted(consumer_keys - set(expected_consumers))} "
        f"arg_drift={sorted(declared_args ^ set(expected_consumers))}"
    )
for key in ("LABBY_BUILDER_IMAGE", "LABBY_RUNTIME_IMAGE"):
    if not re.fullmatch(r"[^@\s]+@sha256:[0-9a-f]{64}", conf.get(key, "")):
        fail(f"{key} is not digest pinned")
for key, value in conf.items():
    if key.endswith("_SHA256") and not re.fullmatch(r"[0-9a-f]{64}", value):
        fail(f"{key} is not a lowercase sha256")
    if key.endswith("_SHA512") and not re.fullmatch(r"[0-9a-f]{128}", value):
        fail(f"{key} is not a lowercase sha512")

dev_required = {
    "LABBY_RUNTIME_IMAGE",
    "LABBY_DEBIAN_SNAPSHOT",
    "LABBY_NODE_VERSION",
    "LABBY_NODE_SHA256",
    "LABBY_UV_VERSION",
    "LABBY_UV_SHA256",
    "LABBY_AGENT_CLIS_LOCK_SHA256",
}
dev_args = dict(
    re.findall(r"(?m)^ARG (LABBY_[A-Z0-9_]+)=([^\s]+)$", dev_dockerfile)
)
if set(dev_args) != dev_required:
    fail(f"development Docker supply keys differ: {sorted(set(dev_args) ^ dev_required)}")
for key in dev_required:
    if dev_args[key] != conf[key] or f"${{{key}}}" not in dev_dockerfile:
        fail(f"development Dockerfile does not consume exact {key}")

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
    {"docker": dict(sorted(conf.items())), "incus": incus},
    sort_keys=True,
    separators=(",", ":"),
).encode()
identity = hashlib.sha256(canonical).hexdigest()
if args.emit_identity:
    print(identity)
