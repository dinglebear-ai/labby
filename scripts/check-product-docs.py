#!/usr/bin/env python3
"""Validate Labby's canonical product documentation and agent-instruction topology."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shlex
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
    "apps/README.md",
    "apps/gateway-admin/README.md",
    "apps/gateway-admin/components/aurora/README.md",
    "apps/palette-tauri/README.md",
    "crates/labby/README.md",
    "crates/labby-apis/README.md",
    "docs/assets/brand/README.md",
    "docs/specs/stdio-mcp-proxy.md",
    "plugins/labby/README.md",
    "LICENSING.md",
    "COMMERCIAL-LICENSING.md",
}

CANONICAL_DIRS = (
    "docs/services/",
    "docs/surfaces/",
    "docs/runtime/",
    "docs/contracts/",
    "docs/guides/",
    "docs/design/",
    "docs/generated/",
    "docs/snippets/",
    "plugins/labby/skills/",
    "apps/gateway-admin/docs/",
)

CANONICAL_DEV = {
    "docs/dev/CODE_MODE.md",
    "docs/dev/DISPATCH.md",
    "docs/dev/ERRORS.md",
    "docs/dev/OBSERVABILITY.md",
    "docs/dev/SERVICE_ONBOARDING.md",
    "docs/dev/SERVICES.md",
    "docs/dev/TESTING.md",
}

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
    "apps/gateway-admin/docs/gateway-detail-redesign.md",
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
    if name in TOP_LEVEL_DOCS or name in CANONICAL_DEV:
        return True
    return name.startswith(CANONICAL_DIRS) and path.suffix.lower() in {".md", ".mdx"}


def canonical_docs() -> list[Path]:
    paths = [p for p in ROOT.rglob("*") if p.is_file() and is_canonical(p)]
    return sorted(paths)


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


def validate_instruction_symlinks(failures: list[str]) -> None:
    ignored_dirs = {
        ".git",
        ".full-review-archive",
        "target",
        "node_modules",
        ".next",
        "out",
    }
    for claude in sorted(ROOT.rglob("CLAUDE.md")):
        if any(part in ignored_dirs for part in claude.parts):
            continue
        directory = claude.parent
        for sibling in ("AGENTS.md", "GEMINI.md"):
            candidate = directory / sibling
            if not candidate.is_symlink():
                failures.append(f"{rel(claude)}: missing {sibling} symlink -> CLAUDE.md")
                continue
            if os.readlink(candidate) != "CLAUDE.md":
                failures.append(
                    f"{rel(candidate)}: expected symlink target CLAUDE.md, got {os.readlink(candidate)!r}"
                )


def validate_auth_bypass_guidance(failures: list[str]) -> None:
    sample = (ROOT / "config/config.example.toml").read_text(encoding="utf-8")
    marker = "# disable_auth = false"
    prefix, found, _ = sample.partition(marker)
    guidance = "\n".join(prefix.splitlines()[-5:]).lower()
    if not found or not all(
        phrase in guidance
        for phrase in ("local development only", "loopback", "must not", "reverse proxy")
    ):
        failures.append(
            "config/config.example.toml: disable_auth must be fenced as loopback-only local development and forbidden behind a reverse proxy"
        )


def validate_install_config_deployment_contracts(failures: list[str]) -> None:
    upstream = (ROOT / "docs/services/UPSTREAM.md").read_text(encoding="utf-8")
    if re.search(r"stdio definitions are marked\s+destructive", upstream, re.IGNORECASE):
        failures.append(
            "docs/services/UPSTREAM.md: stdio administration must not be described as destructive solely because it spawns or mutates restartable state"
        )

    skill_root = ROOT / "plugins/labby/skills/using-labby"
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
            re.findall(r"(?m)^\s+(--[a-z0-9][a-z0-9-]*)(?:\s|$)", body)
        ) | set(re.findall(r"(?m)^\s+(-[A-Za-z0-9])(?:,|\s|$)", body))

    skill_root = ROOT / "plugins/labby/skills/using-labby"
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

    for retired in RETIRED_PATHS:
        if (ROOT / retired).exists() or (ROOT / retired).is_symlink():
            failures.append(f"retired product doc still present: {retired}")

    for path in paths:
        validate_links(path, failures)
        validate_stale_tokens(path, failures)

    validate_duplicates(paths, failures)
    validate_instruction_symlinks(failures)
    validate_auth_bypass_guidance(failures)
    validate_install_config_deployment_contracts(failures)
    validate_shipped_skill_cli_examples(failures)

    if failures:
        print(f"product docs check failed ({len(failures)} issue(s)):", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(f"product docs check passed: {len(paths)} canonical docs; instruction symlinks valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
