#!/usr/bin/env python3
"""Build a disposable SDK-only manifest for the canonical gateway test target.

This does not compile the Labby product. Production Cargo manifests/locks remain
unchanged. The official comparison applies only the SDK 3.4 config-type renames
in copied fixture code, not in product code or the compatibility classifier.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[3]
OFFICIAL = "fd7811fdaa9fefa1c8034534b4d7a31c97204f89"


def value(item):
    if isinstance(item, dict):
        return "{ " + ", ".join(key + " = " + value(val) for key, val in item.items()) + " }"
    return json.dumps(item)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["pin", "official34"])
    args = parser.parse_args()
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    deps = {name: workspace["dependencies"][name] for name in
            ["rmcp", "tokio", "serde_json", "anyhow", "tracing", "wiremock", "rustls"]}
    gateway = tomllib.loads((ROOT / "crates/labby-gateway/Cargo.toml").read_text())
    deps["tokio-util"] = gateway["dependencies"]["tokio-util"]
    harness = ROOT / "target/lane-a" / ("harness-" + args.mode)
    harness.mkdir(parents=True, exist_ok=True)
    test = ROOT / "crates/labby-gateway/tests/issue_771_protocol.rs"
    hashes = {str(test.relative_to(ROOT)): hashlib.sha256(test.read_bytes()).hexdigest()}
    if args.mode == "official34":
        sdk = ROOT / "target/lane-a/rust-sdk"
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=sdk, text=True).strip()
        if actual != OFFICIAL:
            raise SystemExit("official audit checkout must be at " + OFFICIAL)
        deps["rmcp"] = {"path": str(sdk / "crates/rmcp"), "features": deps["rmcp"]["features"]}
        fixture_dir = harness / "issue_771_protocol"
        fixture_dir.mkdir(exist_ok=True)
        for source in sorted(test.with_suffix("").glob("*.rs")):
            hashes[str(source.relative_to(ROOT))] = hashlib.sha256(source.read_bytes()).hexdigest()
            text = source.read_text().replace("ClientInfo", "ClientConfig").replace("ServerInfo", "ServerConfig")
            (fixture_dir / source.name).write_text(text)
        copied = test.read_text()
        test = harness / test.name
        test.write_text(copied)
    manifest = "[package]\nname = \"lane-a-protocol-" + args.mode + "\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\n"
    manifest += "\n".join(name + " = " + value(dep) for name, dep in deps.items())
    manifest += "\n\n[[test]]\nname = \"issue_771_protocol\"\npath = " + value(str(test)) + "\n"
    (harness / "Cargo.toml").write_text(manifest)
    # Reproduce the audit graph without changing production's lockfile.
    if not (harness / "Cargo.lock").exists():
        seed = (Path(__file__).parent / "official34.Cargo.lock"
                if args.mode == "official34" else ROOT / "Cargo.lock")
        shutil.copyfile(seed, harness / "Cargo.lock")
    print(json.dumps({"manifest": str(harness / "Cargo.toml"), "source_sha256": hashes,
                      "comparison_only_type_renames": args.mode == "official34"}, indent=2))


if __name__ == "__main__":
    main()
