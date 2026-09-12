#!/usr/bin/env python3
"""Select the N-1 release that upgrade qualification installs first.

The baseline is the newest release an operator could actually be running: a
published (non-draft, non-prerelease) strict vX.Y.Z release, older than the
candidate and merged into it, that carries every asset the deployment's
installer downloads. A release that fails qualification stays a draft without
assets, so the newest git tag is not a usable baseline: installing it 404s and
strands every later release. Fails closed when nothing qualifies.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

TAG = re.compile(r"v(\d+)\.(\d+)\.(\d+)")


def version(tag: str) -> tuple[int, int, int] | None:
    match = TAG.fullmatch(tag)
    return (int(match[1]), int(match[2]), int(match[3])) if match else None


def releases_from(document: object) -> list[dict]:
    # `gh api --paginate --slurp` wraps each page in an outer array.
    if not isinstance(document, list):
        raise SystemExit("releases document must be a JSON array")
    rows: list[dict] = []
    for item in document:
        rows.extend(item if isinstance(item, list) else [item])
    return rows


def rejection(release: dict, assets: list[str]) -> str | None:
    if release.get("draft") is not False:
        return "draft"
    if release.get("prerelease") is not False:
        return "prerelease"
    uploaded = {row.get("name") for row in release.get("assets", []) if row.get("state") == "uploaded"}
    missing = [name for name in assets if name not in uploaded]
    return f"missing {', '.join(missing)}" if missing else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--candidate", required=True, help="tag under qualification, e.g. v1.16.1")
    parser.add_argument("--releases", type=Path, required=True, help="GitHub releases API JSON")
    parser.add_argument("--merged-tags", type=Path, required=True, help="output of `git tag --merged <candidate>`")
    parser.add_argument("--asset", action="append", required=True, help="asset the baseline must carry (repeatable)")
    args = parser.parse_args()

    candidate = version(args.candidate)
    if candidate is None:
        raise SystemExit(f"candidate {args.candidate!r} is not a vX.Y.Z tag")
    merged = {line.strip() for line in args.merged_tags.read_text().splitlines()}
    older = sorted(
        (tag for tag in merged if (parsed := version(tag)) is not None and parsed < candidate),
        key=version,
        reverse=True,
    )
    by_tag: dict[str, list[dict]] = {}
    for release in releases_from(json.loads(args.releases.read_text())):
        by_tag.setdefault(release.get("tag_name", ""), []).append(release)

    for tag in older:
        reasons = [rejection(release, args.asset) for release in by_tag.get(tag, [])]
        if any(reason is None for reason in reasons):
            print(f"N-1 baseline for {args.candidate}: {tag}", file=sys.stderr)
            print(tag)
            return 0
        # Drafts are invisible to a read-only token, so they show up here too.
        print(f"skip {tag}: {'; '.join(sorted(set(reasons))) or 'no published release'}", file=sys.stderr)
    print(
        f"::error::No published release older than {args.candidate} carries {', '.join(args.asset)}; "
        "N-1 upgrade qualification has no baseline to install.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
