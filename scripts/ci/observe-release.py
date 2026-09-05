#!/usr/bin/env python3
"""Observe every published Labby distribution and emit reconciliation input.

Remote probes intentionally cover gh release, npm, Docker buildx imagetools,
the Incus release asset, and registry.modelcontextprotocol.io/v0.1.
"""
from __future__ import annotations
import argparse, hashlib, json, os, subprocess, urllib.parse
from pathlib import Path

def run(*command: str) -> str:
    return subprocess.check_output(command, text=True, stderr=subprocess.STDOUT).strip()

parser = argparse.ArgumentParser()
parser.add_argument("--manifest", type=Path, required=True)
parser.add_argument("--assets", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
expected = json.loads(args.manifest.read_text())
dist = expected["distributions"]
subjects = []
names = [row["name"] for row in expected["subjects"]]
names += [row["sbom"]["name"] for row in expected["subjects"]]
names += [row["name"] for row in expected.get("auxiliary", [])]
image_sbom = dist["ghcr"].get("sbom")
if image_sbom:
    names.append(image_sbom["name"])
for name in names:
    path = args.assets / name
    if path.is_file():
        subjects.append({"name": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
known_assets = set(names) | {
    "release-manifest.json",
    dist["incus"]["asset"],
    "generation.json",
    "SHA256SUMS",
}
unexpected_assets = sorted(path.name for path in args.assets.iterdir() if path.is_file() and path.name not in known_assets)

attestations = []
verifier = Path(__file__).with_name("verify-release-provenance.sh")
for row in expected.get("attestations", []):
    name = row["subject"]
    path = args.manifest if name == "release-manifest.json" else args.assets / name
    if not path.is_file():
        attestations.append({"subject": name, "status": "missing"})
        continue
    result = subprocess.run(
        [str(verifier), "--repo", expected["repository"], "--workflow", "release.yml",
         "--ref", f'refs/tags/{expected["tag"]}', "--artifact", str(path)],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, check=False,
    )
    attestations.append({"subject": name, "status": "verified" if result.returncode == 0 else "failed"})

observed: dict[str, object] = {}
gh = os.environ.get("GH_BIN", "gh")
npm = os.environ.get("NPM_BIN", "npm")
docker = os.environ.get("DOCKER_BIN", "docker")
curl = os.environ.get("CURL_BIN", "curl")
try:
    is_draft = run(gh, "release", "view", expected["tag"], "--repo", expected["repository"], "--json", "isDraft", "--jq", ".isDraft")
    observed["github"] = dist["github"] if is_draft == "false" else {"isDraft": is_draft}
except Exception as error: observed["github"] = {"error": str(error)}
try:
    npm_tag = dist["npm"].get("tag", "latest")
    version = run(npm, "view", f'{dist["npm"]["package"]}@{npm_tag}', "version", "--json").strip('"')
    observed["npm"] = dist["npm"] if version == dist["npm"]["version"] else {"version": version, "tag": npm_tag}
except Exception as error: observed["npm"] = {"error": str(error)}
try:
    found = run(docker, "buildx", "imagetools", "inspect", f'{dist["ghcr"]["image"]}:{dist["ghcr"]["tag"]}', "--format", "{{json .Manifest.Digest}}").strip('"')
    observed["ghcr"] = dist["ghcr"] if found == dist["ghcr"]["digest"] else {"digest": found}
except Exception as error: observed["ghcr"] = {"error": str(error)}
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
    canonical = json.dumps(server, sort_keys=True, separators=(",", ":")).encode()
    observed["mcp"] = {
        "name": server.get("name"),
        "version": server.get("version"),
        "manifest_sha256": hashlib.sha256(canonical).hexdigest(),
    }
except Exception as error: observed["mcp"] = {"error": str(error)}
args.output.write_text(json.dumps({"subjects": subjects, "unexpected_assets": unexpected_assets, "attestations": attestations, "distributions": observed}, indent=2, sort_keys=True) + "\n")
