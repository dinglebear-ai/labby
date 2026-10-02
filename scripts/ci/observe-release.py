#!/usr/bin/env python3
"""Observe every published Labby distribution and emit reconciliation input.

Remote probes cover gh release, npm, and the distributions named by the immutable manifest.
Legacy manifests include an Incus asset; current Labby releases do not.
"""
from __future__ import annotations
import argparse, hashlib, json, os, subprocess, urllib.parse
from pathlib import Path
from mcp_registry_canonical import manifest_sha256, matches_legacy_manifest

def run(*command: str) -> str:
    return subprocess.check_output(command, text=True, stderr=subprocess.STDOUT).strip()

parser = argparse.ArgumentParser()
parser.add_argument("--manifest", type=Path, required=True)
parser.add_argument("--assets", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--historical", action="store_true", help="Verify immutable npm version availability without requiring the mutable dist-tag to remain on an old release")
parser.add_argument("--mcp-source", type=Path, help="Immutable tagged server.json, used only for legacy raw-hash compatibility")
args = parser.parse_args()
expected = json.loads(args.manifest.read_text())
dist = expected["distributions"]
subjects = []
names = [row["name"] for row in expected["subjects"]]
names += [row["sbom"]["name"] for row in expected["subjects"]]
names += [row["name"] for row in expected.get("auxiliary", [])]
for name in names:
    path = args.assets / name
    if path.is_file():
        subjects.append({"name": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
known_assets = set(names) | {
    "release-manifest.json",
}
known_assets.update(row["name"] for row in expected.get("provenance_bundles", []))
if "incus" in dist:
    known_assets.update({dist["incus"]["asset"], "generation.json", "SHA256SUMS"})
    # Legacy image sidecars were emitted outside the subject manifest. Only
    # recognize their exact names and format under the legacy Incus contract.
    checksum_name = dist["incus"]["asset"] + ".sha256"
    checksum = args.assets / checksum_name
    if checksum.is_file():
        fields = checksum.read_text().split()
        if fields == [dist["incus"]["sha256"], dist["incus"]["asset"]]:
            known_assets.add(checksum_name)
    image_sbom = args.assets / "image.spdx.json"
    checksum_list = args.assets / "SHA256SUMS"
    if image_sbom.is_file() and checksum_list.is_file():
        try:
            checksums = {}
            for line in checksum_list.read_text().splitlines():
                digest, filename = line.split(maxsplit=1)
                filename = filename.lstrip("*")
                if filename in checksums:
                    raise ValueError("duplicate checksum subject")
                checksums[filename] = digest
            bound_digest = checksums.get("image.spdx.json")
            image_bound = checksums.get(dist["incus"]["asset"]) == dist["incus"]["sha256"]
            legacy_sbom = json.loads(image_sbom.read_text())
            if (image_bound
                    and bound_digest == hashlib.sha256(image_sbom.read_bytes()).hexdigest()
                    and legacy_sbom.get("spdxVersion") == "SPDX-2.3"
                    and legacy_sbom.get("SPDXID") == "SPDXRef-DOCUMENT"
                    and isinstance(legacy_sbom.get("packages"), list)):
                known_assets.add("image.spdx.json")
        except (ValueError, AttributeError):
            pass
unexpected_assets = sorted(path.name for path in args.assets.iterdir() if path.is_file() and path.name not in known_assets)

attestations = []
verifier = Path(__file__).with_name("verify-release-provenance.sh")
bundle_names = {row["subject"]: row["name"] for row in expected.get("provenance_bundles", [])}
for row in expected.get("attestations", []):
    name = row["subject"]
    path = args.manifest if name == "release-manifest.json" else args.assets / name
    if not path.is_file():
        attestations.append({"subject": name, "status": "missing"})
        continue
    command = [str(verifier), "--repo", expected["repository"], "--workflow", "release.yml",
               "--ref", f'refs/tags/{expected["tag"]}', "--artifact", str(path)]
    if name in bundle_names:
        bundle = args.assets / bundle_names[name]
        if not bundle.is_file():
            attestations.append({"subject": name, "status": "missing_bundle"})
            continue
        command += ["--bundle", str(bundle)]
    result = subprocess.run(
        command,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, check=False,
    )
    attestations.append({"subject": name, "status": "verified" if result.returncode == 0 else "failed"})

observed: dict[str, object] = {}
gh = os.environ.get("GH_BIN", "gh")
npm = os.environ.get("NPM_BIN", "npm")
curl = os.environ.get("CURL_BIN", "curl")
try:
    is_draft = run(gh, "release", "view", expected["tag"], "--repo", expected["repository"], "--json", "isDraft", "--jq", ".isDraft")
    observed["github"] = dist["github"] if is_draft == "false" else {"isDraft": is_draft}
except Exception as error: observed["github"] = {"error": str(error)}
try:
    npm_tag = dist["npm"].get("tag", "latest")
    selector = dist["npm"]["version"] if args.historical else npm_tag
    version = run(npm, "view", f'{dist["npm"]["package"]}@{selector}', "version", "--json").strip('"')
    observed["npm"] = dist["npm"] if version == dist["npm"]["version"] else {"version": version, "tag": npm_tag}
except Exception as error: observed["npm"] = {"error": str(error)}
if "incus" in dist:
    incus_path = args.assets / dist["incus"]["asset"]
    if incus_path.is_file():
        found = hashlib.sha256(incus_path.read_bytes()).hexdigest()
        observed["incus"] = dist["incus"] if found == dist["incus"]["sha256"] else {"asset": incus_path.name, "sha256": found}
    else: observed["incus"] = {"error": "asset missing"}
try:
    name = urllib.parse.quote(dist["mcp"]["name"], safe="")
    url = f'https://registry.modelcontextprotocol.io/v0.1/servers/{name}/versions/{dist["mcp"]["version"]}'
    payload = json.loads(run(curl, "--fail", "--silent", "--show-error", url))
    server = payload.get("server", payload)
    observed["mcp"] = {
        "name": server.get("name"),
        "version": server.get("version"),
        "manifest_sha256": manifest_sha256(server),
    }
    if (args.historical and args.mcp_source
            and observed["mcp"]["manifest_sha256"] != dist["mcp"]["manifest_sha256"]):
        source = json.loads(args.mcp_source.read_text())
        if matches_legacy_manifest(source, server, dist["mcp"]["manifest_sha256"]):
            raw_source = json.dumps(source, sort_keys=True, separators=(",", ":"))
            observed["mcp"]["manifest_sha256"] = hashlib.sha256(raw_source.encode()).hexdigest()
except Exception as error: observed["mcp"] = {"error": str(error)}
args.output.write_text(json.dumps({"subjects": subjects, "unexpected_assets": unexpected_assets, "attestations": attestations, "distributions": observed}, indent=2, sort_keys=True) + "\n")
