#!/usr/bin/env python3
"""Validate local links in current, maintained Markdown documentation."""

from __future__ import annotations

import html
import re
import subprocess
import sys
import unicodedata
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
ATX_HEADING_RE = re.compile(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$")
SETEXT_HEADING_RE = re.compile(r"^\s{0,3}(?:=+|-+)\s*$")
FENCE_RE = re.compile(r"^\s{0,3}(`{3,}|~{3,})(.*)$")
INLINE_MARKDOWN_LINK_RE = re.compile(r"!?\[([^\]]+)\]\([^)]+\)")
HTML_TAG_RE = re.compile(r"<[^>]*>")
# github-slugger generates its strip regex from Unicode general categories:
# Other_Number; most punctuation including Dash_Punctuation except ASCII '-';
# all Symbol, Control, Private_Use, Format, Unassigned; and all Separator except
# ASCII space. Alphabetic code points are removed from that generated strip set.
# Python's stdlib does not expose the Unicode Alphabetic binary property, so the
# dependency-free implementation below preserves code points Python classifies as
# alphabetic before applying the same category policy.
GITHUB_STRIP_CATEGORIES = {"No", "Pe", "Pf", "Pi", "Ps", "Po", "Pd", "Cc", "Co", "Cf", "Cn"}


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


def github_slugger_removes(char: str) -> bool:
    if char in {" ", "-"} or char.isalpha():
        return False
    category = unicodedata.category(char)
    return (
        category in GITHUB_STRIP_CATEGORIES
        or category.startswith("S")
        or category.startswith("Z")
    )


def github_heading_slug(raw: str) -> str:
    """Return the github-slugger anchor base for a Markdown heading."""
    value = INLINE_MARKDOWN_LINK_RE.sub(lambda match: match.group(1), raw)
    value = HTML_TAG_RE.sub("", value)
    value = html.unescape(value).lower()
    value = "".join(char for char in value if not github_slugger_removes(char))
    return value.replace(" ", "-")


def iter_nonfenced_lines(text: str):
    """Yield Markdown lines outside YAML frontmatter and fenced code blocks."""
    lines = text.splitlines()
    frontmatter_end = 0
    if lines and lines[0].strip() == "---":
        for index, line in enumerate(lines[1:], start=2):
            if line.strip() in {"---", "..."}:
                frontmatter_end = index
                break

    fence_char = ""
    fence_length = 0
    for line_no, line in enumerate(lines, start=1):
        if line_no <= frontmatter_end:
            continue

        fence = FENCE_RE.match(line)
        if fence_char:
            if (
                fence
                and fence.group(1)[0] == fence_char
                and len(fence.group(1)) >= fence_length
                and not fence.group(2).strip()
            ):
                fence_char = ""
                fence_length = 0
            continue

        if fence:
            marker = fence.group(1)
            fence_char = marker[0]
            fence_length = len(marker)
            continue

        yield line_no, line


def heading_anchors(text: str) -> set[str]:
    """Collect GitHub-style ATX and Setext anchors outside fenced code."""
    anchors: set[str] = set()
    next_suffix: dict[str, int] = {}

    def add_heading(raw: str) -> None:
        base = github_heading_slug(raw)
        if not base:
            return
        candidate = base
        suffix = next_suffix.get(base, 0)
        while candidate in anchors:
            suffix += 1
            candidate = f"{base}-{suffix}"
        next_suffix[base] = suffix
        anchors.add(candidate)

    previous: tuple[int, str] | None = None
    for line_no, line in iter_nonfenced_lines(text):
        setext = SETEXT_HEADING_RE.match(line)
        if setext and previous is not None and previous[0] == line_no - 1:
            add_heading(previous[1])
            previous = None
            continue

        atx = ATX_HEADING_RE.match(line)
        if atx:
            add_heading(atx.group(1))
            previous = None
            continue

        previous = (line_no, line.strip()) if line.strip() else None

    return anchors


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


def local_fragment(raw: str) -> tuple[str, str] | None:
    target = strip_title(raw)
    if not target or target.startswith("//"):
        return None
    parsed = urllib.parse.urlsplit(target)
    if parsed.scheme or not parsed.fragment:
        return None
    path = urllib.parse.unquote(parsed.path).strip()
    if path.startswith("/"):
        return None
    return path, urllib.parse.unquote(parsed.fragment).strip()


def fragment_document(source: Path, target_path: str) -> Path | None:
    resolved = source if not target_path else (source.parent / target_path).resolve(strict=False)
    if resolved.is_dir():
        readme = resolved / "README.md"
        return readme if readme.is_file() else None
    if resolved.is_file() and resolved.suffix.lower() in {".md", ".mdx"}:
        return resolved
    return None


def iter_targets(text: str):
    for line_no, line in iter_nonfenced_lines(text):
        for match in INLINE_LINK_RE.finditer(line):
            yield line_no, match.group(1)
        definition = REFERENCE_DEF_RE.match(line)
        if definition:
            yield line_no, definition.group(1)


def main() -> int:
    broken: list[tuple[str, int, str, str]] = []
    checked = 0
    checked_fragments = 0
    anchor_cache: dict[Path, set[str]] = {}

    for path in markdown_paths():
        rel = path.relative_to(ROOT).as_posix()
        text = path.read_text(encoding="utf-8")
        for line_no, raw in iter_targets(text):
            target = local_target(raw)
            if target is not None:
                checked += 1
                resolved = (path.parent / target).resolve(strict=False)
                try:
                    resolved.relative_to(ROOT.resolve())
                except ValueError:
                    broken.append((rel, line_no, raw.strip(), "link escapes repository"))
                    continue
                if not resolved.exists():
                    broken.append((rel, line_no, raw.strip(), "missing local target"))
                    continue

            fragment = local_fragment(raw)
            if fragment is None:
                continue
            target_path, fragment_name = fragment
            fragment_doc = fragment_document(path, target_path)
            if fragment_doc is None:
                continue
            checked_fragments += 1
            anchors = anchor_cache.get(fragment_doc)
            if anchors is None:
                anchors = heading_anchors(fragment_doc.read_text(encoding="utf-8"))
                anchor_cache[fragment_doc] = anchors
            if fragment_name not in anchors:
                broken.append(
                    (rel, line_no, raw.strip(), f"missing Markdown fragment #{fragment_name}")
                )

    if broken:
        print(
            f"documentation link check failed: {len(broken)} broken local link(s)/fragment(s)",
            file=sys.stderr,
        )
        for rel, line_no, raw, reason in broken:
            print(f"  {rel}:{line_no}: {raw} ({reason})", file=sys.stderr)
        return 1

    print(
        "documentation link check passed: "
        f"{checked} local link(s) and {checked_fragments} Markdown fragment(s) verified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
