#!/usr/bin/env python3
"""Create the immutable subject and distribution inventory for a release."""
from __future__ import annotations
import argparse, hashlib, json, re
from pathlib import Path

def digest(path: Path) -> dict[str, object]:
    return {"name": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "size": path.stat().st_size}

parser = argparse.ArgumentParser()
parser.add_argument("--tag", required=True)
parser.add_argument("--repository", required=True)
parser.add_argument("--output", type=Path, default=Path("release-manifest.json"))
parser.add_argument("--image")
parser.add_argument("--image-digest")
parser.add_argument("--image-sbom", type=Path)
parser.add_argument("--npm-package", default="@dinglebear/labby")
parser.add_argument("--incus-asset", default="labby-incus-x86_64-unknown-linux-gnu.tar.xz")
parser.add_argument("--incus-sha256", default="pending")
parser.add_argument("--mcp-name", default="ai.dinglebear/labby")
parser.add_argument("--mcp-manifest-sha256", default="pending")
parser.add_argument("subjects", nargs="+")
args = parser.parse_args()
if bool(args.image) != bool(args.image_digest) or bool(args.image) != bool(args.image_sbom):
    raise SystemExit("image, image-digest, and image-sbom must be provided together")
if args.image_digest and not re.fullmatch(r"sha256:[0-9a-f]{64}", args.image_digest):
    raise SystemExit("image digest must be sha256:<64 lowercase hex>")
for label, value in (("Incus", args.incus_sha256), ("MCP manifest", args.mcp_manifest_sha256)):
    if not re.fullmatch(r"[0-9a-f]{64}", value):
        raise SystemExit(f"{label} digest must be 64 lowercase hex characters")

paths = {Path(name).name: Path(name) for name in args.subjects}
subjects = []
auxiliary = []
for name, path in sorted(paths.items()):
    if name.endswith(".spdx.json"):
        continue
    if name.endswith(".sha256"):
        if not path.is_file():
            raise SystemExit(f"missing release auxiliary: {path}")
        auxiliary.append(digest(path))
        continue
    if not path.is_file():
        raise SystemExit(f"missing release subject: {path}")
    if name.endswith(".tar.gz"):
        sbom_name = name[:-7] + ".spdx.json"
    elif name.endswith(".zip"):
        sbom_name = name[:-4] + ".spdx.json"
    elif name in {"labby-install.sh", "labby-install.ps1"}:
        sbom_name = name + ".spdx.json"
    else:
        raise SystemExit(f"release subject has no SBOM mapping: {name}")
    sbom_path = paths.get(sbom_name)
    if not sbom_path or not sbom_path.is_file():
        raise SystemExit(f"missing SBOM {sbom_name} for {name}")
    row = digest(path)
    row["sbom"] = digest(sbom_path)
    subjects.append(row)

version = args.tag.removeprefix("v")
distributions: dict[str, object] = {
    "github": {"repository": args.repository, "tag": args.tag},
    "npm": {"package": args.npm_package, "version": version, "tag": "latest"},
    "incus": {"asset": args.incus_asset, "sha256": args.incus_sha256},
    "mcp": {"name": args.mcp_name, "version": version, "manifest_sha256": args.mcp_manifest_sha256},
}
if args.image:
    distributions["ghcr"] = {"image": args.image, "tag": args.tag, "digest": args.image_digest, "sbom": digest(args.image_sbom)}
else:
    distributions["ghcr"] = {"identity": "not-configured"}
attested_names = sorted(
    [row["name"] for row in subjects]
    + [row["sbom"]["name"] for row in subjects]
    + [row["name"] for row in auxiliary]
    + ([args.image_sbom.name] if args.image_sbom else [])
    + ["release-manifest.json"]
)
payload = {"schema": "ai.dinglebear.labby/release-manifest/v1", "repository": args.repository, "tag": args.tag, "subjects": subjects, "auxiliary": auxiliary, "attestations": [{"subject": name} for name in attested_names], "distributions": distributions}
args.output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
