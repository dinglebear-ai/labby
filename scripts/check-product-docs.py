#!/usr/bin/env python3
"""Validate Labby's canonical product documentation and agent-instruction topology."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]

TOP_LEVEL_DOCS = {
    "README.md",
    "docs/README.md",
    "docs/ARCH.md",
    "docs/CONVENTIONS.md",
    "docs/OPERATIONS.md",
    "docs/PLUGINS.md",
    "docs/RUST.md",
    "docs/TECH.md",
    "docs/depot-unified-frontend.md",
    "apps/README.md",
    "apps/web/README.md",
    "apps/web/components/aurora/README.md",
    "apps/tauri/README.md",
    "crates/labby/README.md",
    "crates/labby-apis/README.md",
    "docs/assets/brand/README.md",
    "docs/specs/stdio-mcp-proxy.md",
    "plugins/labby/README.md",
}

CANONICAL_DIRS = (
    "docs/access-control/",
    "docs/artifacts/",
    "docs/services/",
    "docs/surfaces/",
    "docs/runtime/",
    "docs/contracts/",
    "docs/guides/",
    "docs/design/",
    "docs/generated/",
    "docs/snippets/",
    "plugins/labby/.apm/skills/",
    "apps/web/docs/",
)

CANONICAL_DEV = {
    "docs/dev/CODE_MODE.md",
    "docs/dev/DISPATCH.md",
    "docs/dev/DEVELOPMENT.md",
    "docs/dev/DOCUMENTATION.md",
    "docs/dev/UPSTREAM_INTERNALS.md",
    "docs/dev/ERRORS.md",
    "docs/dev/OBSERVABILITY.md",
    "docs/dev/RUSTDOC.md",
    "docs/dev/SERVICE_ONBOARDING.md",
    "docs/dev/SERVICES.md",
    "docs/dev/TESTING.md",
    "docs/dev/VERIFICATION.md",
}

MAINTAINED_NONCANONICAL_DIRS = (
    "docs/features/",
    "docs/plans/",
)

IGNORED_PREFIXES = (
    "docs/references/",
    "docs/sessions/",
    "docs/plans/",
)

RETIRED_PATHS = (
    "docs/GATEWAY.md",
    "docs/UPSTREAM.md",
    "docs/coverage",
    "docs/upstream-api",
    "docs/design/CLI_OUTPUT_THEME_API.md",
    "docs/design/cli-output.md",
    "docs/contracts/code-mode-agent-contract-legacy.md",
    "docs/specs/code-mode-spec-legacy.md",
    "docs/specs/gateway-schema-resources.md",
    "docs/dev/SCAFFOLD_AND_AUDIT.md",
    "apps/web/docs/gateway-detail-redesign.md",
)

STALE_PATTERNS = (
    (re.compile(r"crates/lab/"), "use crates/labby/"),
    (re.compile(r"crates/lab-apis/"), "use crates/labby-apis/"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-apis(?![A-Za-z0-9_-])"), "use labby-apis"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-auth(?![A-Za-z0-9_-])"), "use labby-auth"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-codemode(?![A-Za-z0-9_-])"), "use labby-codemode"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-gateway(?![A-Za-z0-9_-])"), "use labby-gateway"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-runtime(?![A-Za-z0-9_-])"), "use labby-runtime"),
    (re.compile(r"(?<![A-Za-z0-9_-])lab-web(?![A-Za-z0-9_-])"), "use labby-web"),
)

LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def is_canonical(path: Path) -> bool:
    name = rel(path)
    if name.startswith(IGNORED_PREFIXES):
        return False
    if path.name == "AGENTS.md" or name in TOP_LEVEL_DOCS or name in CANONICAL_DEV:
        return True
    return name.startswith(CANONICAL_DIRS) and path.suffix.lower() in {".md", ".mdx"}


def repository_paths() -> list[Path]:
    """Return tracked and nonignored new paths, never sibling worktree/cache files.

    Git failure is fatal rather than silently broadening the audit to private or
    generated files. Protected historical trees are excluded before any reads.
    """
    raw = subprocess.check_output(
        ["git", "-C", str(ROOT), "ls-files", "--cached", "--others", "--exclude-standard", "-z"]
    )
    excluded = (
        ".full-review-archive/", "docs/archive/", "docs/references/",
        "docs/sessions/", "docs/superpowers/", "vendor/",
    )
    return sorted({
        ROOT / value.decode("utf-8")
        for value in raw.split(bytes([0]))
        if value and not value.decode("utf-8").startswith(excluded)
        and ((ROOT / value.decode("utf-8")).exists() or (ROOT / value.decode("utf-8")).is_symlink())
    })


def canonical_docs() -> list[Path]:
    return [p for p in repository_paths() if not p.is_symlink() and p.is_file() and is_canonical(p)]


def maintained_noncanonical_docs() -> list[Path]:
    return [
        p
        for p in repository_paths()
        if not p.is_symlink() and p.is_file()
        and p.suffix.lower() in {".md", ".mdx"}
        and rel(p).startswith(MAINTAINED_NONCANONICAL_DIRS)
    ]


def strip_link_target(raw: str) -> str:
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    if ' "' in target:
        target = target.split(' "', 1)[0]
    elif " '" in target:
        target = target.split(" '", 1)[0]
    return unquote(target)


def validate_links(path: Path, failures: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    for match in LINK_RE.finditer(text):
        target = strip_link_target(match.group(1))
        if not target or target.startswith(("#", "/", "http://", "https://", "mailto:", "data:")):
            continue
        if "${" in target or "{{" in target:
            continue
        file_part = target.split("#", 1)[0].split("?", 1)[0]
        if not file_part:
            continue
        resolved = (path.parent / file_part).resolve()
        try:
            resolved.relative_to(ROOT.resolve())
        except ValueError:
            failures.append(f"{rel(path)}: link escapes repository: {target}")
            continue
        if not resolved.exists():
            line = text.count("\n", 0, match.start()) + 1
            failures.append(f"{rel(path)}:{line}: missing local link target: {target}")


def validate_stale_tokens(path: Path, failures: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    for pattern, guidance in STALE_PATTERNS:
        for match in pattern.finditer(text):
            line = text.count("\n", 0, match.start()) + 1
            failures.append(f"{rel(path)}:{line}: stale product naming {match.group(0)!r}; {guidance}")


def validate_duplicates(paths: list[Path], failures: list[str]) -> None:
    groups: dict[str, list[str]] = {}
    for path in paths:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        groups.setdefault(digest, []).append(rel(path))
    for names in groups.values():
        if len(names) > 1:
            failures.append("duplicate canonical product docs: " + ", ".join(sorted(names)))


def service_names_from_table(text: str, heading: str | None = None) -> set[str]:
    """Return backticked service ids from the first column of one Markdown table."""
    if heading is not None:
        match = re.search(rf"(?m)^## {re.escape(heading)}\s*$", text)
        if match is None:
            return set()
        text = text[match.end() :]

    names: set[str] = set()
    in_table = False
    for line in text.splitlines():
        if not line.startswith("|"):
            if in_table:
                break
            continue
        in_table = True
        cells = line.split("|")
        if len(cells) < 3:
            continue
        names.update(re.findall(r"`([a-z][a-z0-9_]*)`", cells[1]))
    return names


def validate_service_index_coverage(failures: list[str]) -> None:
    catalog = json.loads(
        (ROOT / "docs/generated/service-catalog.json").read_text(encoding="utf-8")
    )
    service_names = {entry["name"] for entry in catalog}
    indexes = (
        ("docs/README.md", "Current Product Services"),
        ("docs/services/README.md", None),
    )
    for relative_path, heading in indexes:
        text = (ROOT / relative_path).read_text(encoding="utf-8")
        indexed = service_names_from_table(text, heading)
        missing = sorted(service_names - indexed)
        unknown = sorted(indexed - service_names)
        if missing:
            failures.append(
                f"{relative_path}: registered services missing from documentation index table: "
                + ", ".join(missing)
            )
        if unknown:
            failures.append(
                f"{relative_path}: unknown or retired services present in documentation index table: "
                + ", ".join(unknown)
            )


def validate_instruction_symlinks(failures: list[str]) -> None:
    # Inventory every alias as well as the canonical name: an orphan CLAUDE.md
    # must fail even when its AGENTS.md has been removed. Always require root.
    names = {"AGENTS.md", "CLAUDE.md", "GEMINI.md"}
    private_names = {
        "AGENTS.override.md", "CLAUDE.local.md", "AGENTS.local.md",
        "AGENTS.md.local", "CLAUDE.md.local",
    }
    paths = repository_paths()
    for path in paths:
        if path.name in private_names:
            failures.append(f"{rel(path)}: private instructions must remain Git-ignored and untracked")
    directories = {ROOT}
    directories.update(path.parent for path in paths if path.name in names)
    for directory in sorted(directories):
        agents = directory / "AGENTS.md"
        if agents.is_symlink() or not agents.is_file():
            failures.append(f"{rel(agents)}: canonical instructions must be a regular file")
        elif not agents.read_text(encoding="utf-8").strip():
            failures.append(f"{rel(agents)}: canonical instructions must not be empty")
        for sibling in ("CLAUDE.md", "GEMINI.md"):
            candidate = directory / sibling
            if not candidate.is_symlink():
                failures.append(f"{rel(candidate)}: missing symlink -> AGENTS.md")
                continue
            if os.readlink(candidate) != "AGENTS.md":
                failures.append(
                    f"{rel(candidate)}: expected symlink target AGENTS.md, got {os.readlink(candidate)!r}"
                )


def validate_instruction_budgets(failures: list[str]) -> None:
    """Bound root characters and repository-only UTF-8 instruction chains.

    Private/global instructions and dynamic imports are not read by this gate.
    Two separator bytes per boundary conservatively account for concatenation.
    """
    guides: dict[Path, bytes] = {}
    for path in repository_paths():
        if path.name != "AGENTS.md" or path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            failures.append(f"{rel(path)}: instructions must be UTF-8")
            continue
        if path == ROOT / "AGENTS.md" and len(text) > 7900:
            failures.append(f"AGENTS.md: {len(text)} characters exceeds the 7900-character limit")
        guides[path.parent] = data
    for directory in sorted(guides):
        chain = [parent for parent in [directory, *directory.parents] if parent in guides]
        total = sum(len(guides[parent]) for parent in chain) + max(0, len(chain) - 1) * 2
        if total > 32 * 1024:
            failures.append(
                f"{rel(directory / 'AGENTS.md')}: repository instruction chain is {total} UTF-8 bytes; "
                "exceeds 32768 bytes, move detailed references out of startup instructions"
            )


def validate_auth_bypass_guidance(failures: list[str]) -> None:
    sample = (ROOT / ".config/config.example.toml").read_text(encoding="utf-8")
    marker = "# disable_auth = false"
    prefix, found, _ = sample.partition(marker)
    guidance = "\n".join(prefix.splitlines()[-5:]).lower()
    if not found or not all(
        phrase in guidance
        for phrase in ("local development only", "loopback", "must not", "reverse proxy")
    ):
        failures.append(
            ".config/config.example.toml: disable_auth must be fenced as loopback-only local development and forbidden behind a reverse proxy"
        )


def validate_install_config_deployment_contracts(failures: list[str]) -> None:
    upstream = (ROOT / "docs/services/UPSTREAM.md").read_text(encoding="utf-8")
    if re.search(r"stdio definitions are marked\s+destructive", upstream, re.IGNORECASE):
        failures.append(
            "docs/services/UPSTREAM.md: stdio administration must not be described as destructive solely because it spawns or mutates restartable state"
        )

    skill_root = ROOT / "plugins/labby/.apm/skills/using-labby"
    retired = re.compile(
        r"\b(?:marketplace|service[ _.-]?deploy|deploy product)\b|"
        r'\"service\"\s*:\s*\"deploy\"',
        re.IGNORECASE,
    )
    for path in sorted(skill_root.rglob("*.md")):
        text = path.read_text(encoding="utf-8")
        match = retired.search(text)
        if match:
            line = text.count("\n", 0, match.start()) + 1
            failures.append(
                f"{rel(path)}:{line}: shipped operator skill references retired product surface {match.group(0)!r}"
            )

    host = (ROOT / "docs/runtime/HOST_GATEWAY.md").read_text(encoding="utf-8").lower()
    for package in ("jq", "ripgrep", "lsof", "rsync", "python3", "ffmpeg", "adb"):
        if package not in host:
            failures.append(
                f"docs/runtime/HOST_GATEWAY.md: default provisioning package summary omits {package}"
            )

    cicd = (ROOT / "docs/runtime/CICD.md").read_text(encoding="utf-8")
    for match in re.finditer(r"(?<![A-Za-z0-9_-])lab (?:package|binary)", cicd):
        line = cicd.count("\n", 0, match.start()) + 1
        failures.append(
            f"docs/runtime/CICD.md:{line}: release docs must name the labby package/binary; lab is protocol compatibility vocabulary"
        )


def validate_shipped_skill_cli_examples(failures: list[str]) -> None:
    """Reject single-line Labby examples that use flags absent from Clap help."""
    help_text = (ROOT / "docs/generated/cli-help.md").read_text(encoding="utf-8")
    sections = list(re.finditer(r"(?m)^## `(?P<command>labby(?: [^`]+)?)`\n", help_text))
    commands: dict[tuple[str, ...], set[str]] = {}
    for index, section in enumerate(sections):
        end = sections[index + 1].start() if index + 1 < len(sections) else len(help_text)
        body = help_text[section.end() : end]
        commands[tuple(section.group("command").split())] = set(
            re.findall(
                r"(?m)^[ \t]+(?:-[A-Za-z0-9], )?(--[a-z0-9][a-z0-9-]*)(?:\.\.\.)?(?:\s|$)",
                body,
            )
        ) | set(re.findall(r"(?m)^\s+(-[A-Za-z0-9])(?:,|\s|$)", body))

    skill_root = ROOT / "plugins/labby/.apm/skills/using-labby"
    for path in sorted(skill_root.rglob("*.md")):
        in_bash = False
        for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            stripped = raw_line.strip()
            if stripped == "```bash":
                in_bash = True
                continue
            if stripped == "```" and in_bash:
                in_bash = False
                continue
            if not in_bash or not stripped.startswith("labby ") or stripped.endswith("\\"):
                continue
            try:
                tokens = shlex.split(stripped)
            except ValueError as error:
                failures.append(f"{rel(path)}:{line_number}: invalid shell example: {error}")
                continue
            command = max(
                (candidate for candidate in commands if tokens[: len(candidate)] == list(candidate)),
                key=len,
                default=None,
            )
            if command is None:
                failures.append(f"{rel(path)}:{line_number}: unknown Labby CLI command")
                continue
            allowed = commands[command]
            for token in tokens[len(command) :]:
                option = token.split("=", 1)[0]
                if option.startswith("-") and option not in allowed:
                    failures.append(
                        f"{rel(path)}:{line_number}: {option} is not accepted by {' '.join(command)}"
                    )


def main() -> int:
    failures: list[str] = []
    paths = canonical_docs()
    maintained_noncanonical = maintained_noncanonical_docs()

    owned = {rel(path) for path in repository_paths()}
    for retired in RETIRED_PATHS:
        present = any(name == retired or name.startswith(retired + "/") for name in owned)
        if present and ((ROOT / retired).exists() or (ROOT / retired).is_symlink()):
            failures.append(f"retired product doc still present: {retired}")

    for path in [*paths, *maintained_noncanonical]:
        validate_links(path, failures)
        validate_stale_tokens(path, failures)

    validate_duplicates(paths, failures)
    validate_service_index_coverage(failures)
    validate_instruction_symlinks(failures)
    validate_instruction_budgets(failures)
    validate_auth_bypass_guidance(failures)
    validate_install_config_deployment_contracts(failures)
    validate_shipped_skill_cli_examples(failures)

    if failures:
        print(f"product docs check failed ({len(failures)} issue(s)):", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(
        "product docs check passed: "
        f"{len(paths)} canonical docs, {len(maintained_noncanonical)} maintained planning/feature docs; "
        "service indexes and instruction symlinks valid"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
