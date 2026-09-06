#!/usr/bin/env python3
"""Validate local links in current, maintained Markdown documentation."""

from __future__ import annotations

import re
import subprocess
import sys
import urllib.parse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKIP_PREFIXES = (
    # Immutable full-review snapshots preserve partial source trees for audit
    # evidence; they are not maintained product-documentation roots.
    ".full-review-archive/",
    "docs/archive/",
    "docs/sessions/",
    "docs/superpowers/",
    # Vendored dependency prose preserves links from its upstream workspace;
    # product documentation checks do not own or rewrite those snapshots.
    "vendor/",
)
SKIP_FILES = {
    "CHANGELOG.md",
    # This package README is intentionally synced byte-for-byte from the root
    # README before packing, so its repository-relative links are rooted at the
    # product repository rather than the package directory.
    "packages/labby-mcp/README.md",
}
SKIP_NAMES = {"AGENTS.md", "GEMINI.md"}
INLINE_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
REFERENCE_DEF_RE = re.compile(r"^\s{0,3}\[[^\]]+\]:\s*(\S.*)$")
TITLE_RE = re.compile(r"\s+(?:\"[^\"]*\"|'[^']*'|\([^)]*\))\s*$")


def markdown_paths() -> list[Path]:
    raw = subprocess.check_output(
        [
            "git",
            "-C",
            str(ROOT),
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*.md",
            "*.mdx",
        ]
    )
    paths: list[Path] = []
    for value in raw.split(b"\0"):
        if not value:
            continue
        rel = value.decode("utf-8")
        if rel in SKIP_FILES or rel.startswith(SKIP_PREFIXES):
            continue
        path = ROOT / rel
        if path.name in SKIP_NAMES or path.is_symlink() or not path.exists():
            continue
        paths.append(path)
    return sorted(paths)


def strip_title(raw: str) -> str:
    value = raw.strip()
    if value.startswith("<"):
        end = value.find(">")
        if end != -1:
            return value[1:end].strip()
    return TITLE_RE.sub("", value).strip()


def local_target(raw: str) -> str | None:
    target = strip_title(raw)
    if not target or target.startswith("#") or target.startswith("//"):
        return None
    parsed = urllib.parse.urlsplit(target)
    if parsed.scheme:
        return None
    path = urllib.parse.unquote(parsed.path).strip()
    if not path or path.startswith("/"):
        # Absolute paths in product prose are web/application routes, not
        # repository-local documentation links.
        return None
    return path


def iter_targets(text: str):
    fenced = False
    fence_marker = ""
    for line_no, line in enumerate(text.splitlines(), start=1):
        stripped = line.lstrip()
        if stripped.startswith((chr(96) * 3, "~~~")):
            marker = stripped[:3]
            if not fenced:
                fenced = True
                fence_marker = marker
            elif marker == fence_marker:
                fenced = False
                fence_marker = ""
            continue
        if fenced:
            continue
        for match in INLINE_LINK_RE.finditer(line):
            yield line_no, match.group(1)
        definition = REFERENCE_DEF_RE.match(line)
        if definition:
            yield line_no, definition.group(1)


def main() -> int:
    broken: list[tuple[str, int, str]] = []
    checked = 0
    for path in markdown_paths():
        rel = path.relative_to(ROOT).as_posix()
        text = path.read_text(encoding="utf-8")
        for line_no, raw in iter_targets(text):
            target = local_target(raw)
            if target is None:
                continue
            checked += 1
            resolved = (path.parent / target).resolve(strict=False)
            try:
                resolved.relative_to(ROOT.resolve())
            except ValueError:
                broken.append((rel, line_no, raw.strip()))
                continue
            if not resolved.exists():
                broken.append((rel, line_no, raw.strip()))

    if broken:
        print(f"documentation link check failed: {len(broken)} broken local link(s)", file=sys.stderr)
        for rel, line_no, raw in broken:
            print(f"  {rel}:{line_no}: {raw}", file=sys.stderr)
        return 1

    print(f"documentation link check passed: {checked} local link(s) verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
