#!/usr/bin/env python3
"""Extract the pinned MCP 2026-07-28 BCP 14 requirement denominator."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
from collections import defaultdict
from pathlib import Path

PROTOCOL_VERSION = "2026-07-28"
SOURCE_REVISION = "5f5440bb26a62e2cf3440b92da5a667efa03b267"
REPOSITORY = "https://github.com/modelcontextprotocol/modelcontextprotocol"
EXPECTED_PAGE_COUNT = 31
MODAL = re.compile(
    r"(?<![A-Za-z])(?:\*\*)?(MUST NOT|SHOULD NOT|NOT RECOMMENDED|SHALL NOT|"
    r"REQUIRED|MUST|SHOULD|SHALL|MAY|RECOMMENDED|OPTIONAL)(?:\*\*)?(?![A-Za-z])"
)
STRENGTH = {
    "MUST": "must",
    "MUST NOT": "must_not",
    "SHOULD": "should",
    "SHOULD NOT": "should_not",
    "MAY": "may",
    "RECOMMENDED": "should",
    "NOT RECOMMENDED": "should_not",
    "REQUIRED": "must",
    "SHALL": "must",
    "SHALL NOT": "must_not",
    "OPTIONAL": "may",
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def normalize(text: str) -> str:
    return re.sub(r"\s+", " ", re.sub(r"[*`_]", "", text)).strip().lower()


def anchor_for(heading: str) -> str:
    value = re.sub(r"<[^>]+>", "", heading.lower())
    value = re.sub(r"[^a-z0-9 _-]", "", value).strip()
    return "#" + re.sub(r"[ _]+", "-", value)


def paragraph_units(lines: list[str]) -> list[tuple[int, int, str, str]]:
    """Return non-code prose units with inclusive lines and nearest heading."""
    units: list[tuple[int, int, str, str]] = []
    current: list[str] = []
    start = 0
    heading = "top"
    in_fence = False

    def flush() -> None:
        nonlocal current, start
        if current:
            units.append((start, start + len(current) - 1, "".join(current), heading))
            current = []
            start = 0

    for number, line in enumerate(lines, 1):
        if line.lstrip().startswith("```"):
            flush()
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        match = re.match(r"^#{1,6}\s+(.+)", line)
        if match:
            flush()
            heading = re.sub(r"[*`]", "", match.group(1).strip())
            continue
        if line.lstrip().startswith("|"):
            flush()
            units.append((number, number, line, heading))
            continue
        if (
            not line.strip()
            or line.lstrip().startswith("import ")
            or line.strip() == "---"
            or (line.lstrip().startswith("<") and line.rstrip().endswith("/>"))
        ):
            flush()
            continue
        # A same-level list item with its own BCP 14 keyword starts a new
        # obligation; subordinate non-modal items remain attached to a colon
        # leader so they can inherit that leader's strength.
        if current and re.match(r"^\s*(?:[-*]|\d+\.)\s+", line) and MODAL.search(line):
            flush()
        if not current:
            start = number
        current.append(line)
    flush()
    return units


def attach_colon_lists(
    units: list[tuple[int, int, str, str]], lines: list[str] | None = None,
) -> list[tuple[int, int, str, str]]:
    """Attach Markdown list units to a preceding modal leader ending in a colon."""
    attached: list[tuple[int, int, str, str]] = []
    index = 0
    while index < len(units):
        start, end, text, heading = units[index]
        if MODAL.search(text) and text.rstrip().endswith(":"):
            pieces = [text]
            cursor = index + 1
            while cursor < len(units) and re.match(r"^\s*(?:[-*]|\d+\.)\s+", units[cursor][2]):
                if lines is not None and any(line.strip() for line in lines[end : units[cursor][0] - 1]):
                    break
                end = units[cursor][1]
                pieces.append(units[cursor][2])
                cursor += 1
            if cursor > index + 1:
                attached.append((start, end, "".join(pieces), heading))
                index = cursor
                continue
        attached.append(units[index])
        index += 1
    return attached


def sentence_units(text: str) -> list[str]:
    listed = re.sub(r"\n(?=\s*[-*]\s+)", "\0", text)
    return [
        part
        for part in re.split(r"\0|(?<=[.!?])\s+(?=(?:[A-Z*`]|\d+\.))", listed)
        if part.strip()
    ]


def split_modal_clauses(sentence: str) -> list[tuple[str, str, int]]:
    """Split independently worded modal clauses without duplicating a sentence."""
    matches = list(MODAL.finditer(sentence))
    if not matches:
        return []
    starts = [0]
    ends: list[int] = []
    for left, right in zip(matches, matches[1:]):
        between = sentence[left.end() : right.start()]
        separators = list(re.finditer(r";\s*(?:otherwise,?\s*)?|,\s+(?:and|but)\s+|\s+and\s+|\|", between))
        if separators:
            separator = separators[-1]
            cut_start = left.end() + separator.start()
            cut_end = left.end() + separator.end()
        else:
            punctuation = max(between.rfind(":"), between.rfind(","))
            cut_start = left.end() + punctuation + 1 if punctuation >= 0 else right.start()
            cut_end = cut_start
        ends.append(cut_start)
        starts.append(cut_end)
    ends.append(len(sentence))
    clauses = []
    for index, (match, start, end) in enumerate(zip(matches, starts, ends)):
        clause = sentence[start:end].strip()
        if clause:
            clauses.append((clause, match.group(1), index))
    return clauses


def inherited_list_clauses(paragraph: str) -> list[tuple[str, str, int]] | None:
    """Expand a modal ``...:`` leader into each operative Markdown list item."""
    lines = paragraph.splitlines()
    if not lines:
        return None
    leader_end = next((index for index, line in enumerate(lines) if line.rstrip().endswith(":") and MODAL.search(" ".join(lines[: index + 1]))), None)
    if leader_end is None:
        return None
    leader = " ".join(line.strip() for line in lines[: leader_end + 1]).rstrip(":")
    modal = MODAL.search(leader)
    assert modal is not None
    items: list[str] = []
    current: list[str] = []
    for line in lines[leader_end + 1 :]:
        item = re.match(r"^\s*(?:[-*]|\d+\.)\s+(.+)", line)
        if item:
            if current:
                items.append(" ".join(current))
            current = [item.group(1).strip()]
        elif current and line.strip():
            current.append(line.strip())
    if current:
        items.append(" ".join(current))
    if not items:
        return None
    return [(f"{leader} {item}", modal.group(1), index) for index, item in enumerate(items)]


def infer_roles(clause: str) -> tuple[list[str], str]:
    match = MODAL.search(clause)
    actor = clause[: match.start()] if match else clause
    actor = normalize(actor)
    roles: list[str] = []
    if "authorization server" in actor:
        roles.append("authorization_server")
    if re.search(r"(?:mcp |resource )?servers?\b", actor) and "authorization server" not in actor:
        roles.append("server")
    if re.search(r"(?:mcp )?clients?\b", actor):
        roles.append("client")
    if re.search(r"\bhosts?\b", actor):
        roles.append("host")
    if re.search(r"\b(?:proxy|proxies|intermediar(?:y|ies))\b", actor):
        roles.append("proxy")
    return (roles, "explicit_actor") if roles else (["unspecified"], "unspecified_actor")


def capability_for(path: str) -> str:
    names = (
        "authorization", "tools", "resources", "prompts", "sampling", "elicitation",
        "roots", "progress", "cancellation", "subscriptions", "caching", "completion",
        "logging", "pagination",
    )
    for name in names:
        if name in path:
            return name
    if "transport" in path:
        return "transport"
    if "versioning" in path or "discover" in path:
        return "lifecycle"
    return "core"


def transport_for(path: str, clause: str) -> list[str]:
    lowered = clause.lower()
    transports = []
    if "stdio" in path or "stdio" in lowered:
        transports.append("stdio")
    if "streamable-http" in path or "http" in lowered:
        transports.append("http")
    return transports or ["all"]


def reference_definitions(lines: list[str]) -> dict[str, str]:
    definitions = {}
    for line in lines:
        match = re.match(r"^\s*\[([^]]+)\]:\s*(?:<)?([^\s>]+)", line)
        if match:
            definitions[match.group(1).casefold()] = match.group(2)
    return definitions


def normative_reference_classification(
    clause: str, definitions: dict[str, str] | None = None
) -> tuple[str, list[str]]:
    references = re.findall(r"\[[^]]+\]\((https?://[^)]+)\)", clause)
    references += re.findall(r"<(?:a|Link)\b[^>]*\bhref=[\"'](https?://[^\"']+)", clause, re.IGNORECASE)
    definitions = definitions or {}
    for label in re.findall(r"\[[^]]+\]\[([^]]+)\]", clause):
        target = definitions.get(label.casefold())
        if target and target.startswith(("http://", "https://")):
            references.append(target)
        elif target is None and re.search(r"(?:rfc|ietf|openid|oauth)", label, re.IGNORECASE):
            references.append(f"unresolved-reference:{label}")
    for number in re.findall(r"(?<![A-Za-z])RFC\s*([0-9]{3,5})(?![0-9])", clause, re.IGNORECASE):
        if not any(re.search(rf"rfc{number}(?:\D|$)", url, re.IGNORECASE) for url in references):
            references.append(f"https://www.rfc-editor.org/rfc/rfc{number}.html")
    references = list(dict.fromkeys(references))
    incorporates = bool(re.search(
        r"\b(?:follow|following|implement|conform(?:s|ing)?|as (?:required|specified|defined) (?:by|in)|according to|per)\b",
        clause,
        re.IGNORECASE,
    )) and bool(references)
    return ("external_reference" if incorporates else "direct", references if incorporates else [])


def load_auth_rows(path: Path) -> dict[str, list[dict]]:
    if not path.exists():
        return {}
    rows = json.loads(path.read_text())["requirements"]
    grouped: dict[str, list[dict]] = defaultdict(list)
    prefix = f"https://modelcontextprotocol.io/specification/{PROTOCOL_VERSION}/"
    for row in rows:
        grouped[row["source_url"].removeprefix(prefix).removesuffix(".md") + ".mdx"].append(row)
    return grouped


def exact_auth_id(clause: str, strength: str, rows: list[dict], used: set[str]) -> list[str]:
    normalized = normalize(clause)
    candidates = []
    for row in rows:
        asserted = normalize(row["requirement"])
        if row["id"] not in used and row["strength"] == strength and asserted in normalized:
            candidates.append((normalized.index(asserted), row["id"]))
    if not candidates:
        return []
    identifier = min(candidates)[1]
    used.add(identifier)
    return [identifier]


def verify_pinned_pages(source_root: Path, files: list[Path]) -> None:
    """Bind extraction to the exact committed bytes at the pinned revision."""
    revision = subprocess.run(
        ["git", "-C", str(source_root), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if revision != SOURCE_REVISION:
        raise ValueError(f"source checkout is {revision}, expected {SOURCE_REVISION}")
    for source in files:
        relative = source.relative_to(source_root).as_posix()
        pinned = subprocess.run(
            ["git", "-C", str(source_root), "show", f"{SOURCE_REVISION}:{relative}"],
            check=True,
            capture_output=True,
        ).stdout
        if source.read_bytes() != pinned:
            raise ValueError(f"source page differs from pinned Git blob: {relative}")


def extract(source_root: Path, auth_path: Path) -> tuple[dict, dict]:
    spec_root = source_root / "docs" / "specification" / PROTOCOL_VERSION
    files = sorted(spec_root.rglob("*.mdx"))
    if len(files) != EXPECTED_PAGE_COUNT:
        raise ValueError(f"found {len(files)} dated pages, expected {EXPECTED_PAGE_COUNT}")
    verify_pinned_pages(source_root, files)
    auth_rows = load_auth_rows(auth_path)
    used_auth: set[str] = set()
    requirements = []
    pages = []
    for source in files:
        relative = source.relative_to(source_root).as_posix()
        short = source.relative_to(spec_root).as_posix()
        lines = source.read_text().splitlines(keepends=True)
        definitions = reference_definitions(lines)
        page_count = 0
        for start, end, paragraph, heading in attach_colon_lists(paragraph_units(lines), lines):
            if not MODAL.search(paragraph):
                continue
            if 'The key words "MUST"' in paragraph and "interpreted as described in" in paragraph:
                continue
            inherited = inherited_list_clauses(paragraph)
            clause_groups = [inherited] if inherited else [split_modal_clauses(sentence) for sentence in sentence_units(paragraph)]
            for clauses in clause_groups:
                for clause, modal, _ordinal in clauses:
                    page_count += 1
                    path_slug = re.sub(r"[^A-Za-z0-9]+", "-", short.removesuffix(".mdx")).upper()
                    strength = STRENGTH[modal]
                    roles, role_inference = infer_roles(clause)
                    requirement_kind, external_references = normative_reference_classification(clause, definitions)
                    source_slice = "".join(lines[start - 1 : end]).encode()
                    requirements.append({
                        "id": f"MCP-2026-SPEC-{path_slug}-{page_count:03d}",
                        "source_path": relative,
                        "source_url": f"{REPOSITORY}/blob/{SOURCE_REVISION}/{relative}",
                        "source_anchor": anchor_for(heading),
                        "source_lines": [start, end],
                        "source_sha256": sha256(source_slice),
                        "strength": strength,
                        "requirement": clause,
                        "roles": roles,
                        "role_inference": role_inference,
                        "transports": transport_for(short, clause),
                        "capability": capability_for(short),
                        "requirement_kind": requirement_kind,
                        "external_normative_references": external_references,
                        "applicability": "unreviewed",
                        "applicability_reason": "Normative source obligation inventoried; Labby applicability is assigned by the host mapping stage.",
                        "existing_auth_ids": exact_auth_id(clause, strength, auth_rows.get(short, []), used_auth),
                    })
        pages.append({
            "path": relative,
            "url": f"{REPOSITORY}/blob/{SOURCE_REVISION}/{relative}",
            "sha256": sha256(source.read_bytes()),
            "classification": "normative" if page_count else "nonnormative",
            "normative_count": page_count,
        })
    return ({
        "schema_version": 1,
        "protocol_version": PROTOCOL_VERSION,
        "repository": REPOSITORY,
        "revision": SOURCE_REVISION,
        "pages": pages,
    }, {
        "schema_version": 1,
        "protocol_version": PROTOCOL_VERSION,
        "source_revision": SOURCE_REVISION,
        "requirements": requirements,
    })


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--sources-output", type=Path, default=Path("conformance/mcp-spec-sources.json"))
    parser.add_argument("--requirements-output", type=Path, default=Path("conformance/mcp-spec-requirements.json"))
    parser.add_argument("--auth-requirements", type=Path, default=Path("conformance/mcp-auth-normative.json"))
    args = parser.parse_args()
    sources, requirements = extract(args.source_root, args.auth_requirements)
    args.sources_output.write_text(json.dumps(sources, indent=2) + "\n")
    args.requirements_output.write_text(json.dumps(requirements, indent=2) + "\n")


if __name__ == "__main__":
    main()
