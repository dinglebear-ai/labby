#!/usr/bin/env python3
"""Select a published ARM64 baseline or the first-release source bootstrap."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import runpy
import sys

_baseline = runpy.run_path(str(Path(__file__).with_name("resolve-n-minus-one-baseline.py")))
releases_from = _baseline["releases_from"]
version = _baseline["version"]

ARM_ARCHIVE = "lab-aarch64-unknown-linux-gnu.tar.gz"
X86_ARCHIVE = "lab-x86_64-unknown-linux-gnu.tar.gz"


def select(candidate: str, releases: object, merged_tags: set[str]) -> tuple[str, str]:
    candidate_version = version(candidate)
    if candidate_version is None:
        raise ValueError(f"candidate {candidate!r} is not a vX.Y.Z tag")
    by_tag: dict[str, list[set[str]]] = {}
    for release in releases_from(releases):
        if release.get("draft") is not False or release.get("prerelease") is not False:
            continue
        assets = {row.get("name") for row in release.get("assets", []) if row.get("state") == "uploaded"}
        by_tag.setdefault(release.get("tag_name", ""), []).append(assets)
    older = sorted(
        (tag for tag in merged_tags if (parsed := version(tag)) is not None and parsed < candidate_version),
        key=version, reverse=True,
    )
    published = [(tag, assets) for tag in older for assets in by_tag.get(tag, [])]
    # A previously published ARM archive makes bootstrap permanently ineligible.
    # A missing checksum on that archive is a release-integrity failure, not a
    # reason to substitute a source-built baseline.
    arm = [(tag, assets) for tag, assets in published if ARM_ARCHIVE in assets]
    if arm:
        tag, assets = arm[0]
        if ARM_ARCHIVE + ".sha256" not in assets:
            raise ValueError(f"published ARM64 baseline {tag} lacks its checksum sidecar")
        return "published", tag
    for tag, assets in published:
        if {X86_ARCHIVE, X86_ARCHIVE + ".sha256"} <= assets:
            return "bootstrap", tag
    raise ValueError("no published stable baseline has a complete Linux archive for ARM64 bootstrap")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--releases", required=True, type=Path)
    parser.add_argument("--merged-tags", required=True, type=Path)
    args = parser.parse_args()
    try:
        mode, tag = select(args.candidate, json.loads(args.releases.read_text()),
                           set(args.merged_tags.read_text().splitlines()))
    except (ValueError, KeyError, TypeError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    print(f"{mode} {tag}")
    print(f"ARM64 N-1 {mode} baseline: {tag}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
