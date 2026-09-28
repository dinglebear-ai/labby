#!/usr/bin/env python3
"""Run the pinned fleet contract with Labby's AGENTS-first index policy."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
from pathlib import Path, PurePosixPath
import subprocess
import sys
from types import ModuleType

# The adapter is reviewed against this exact implementation. A fleet upgrade
# must deliberately review the compatibility policy and update this digest.
FLEET_SHA256 = "bc468fbd233c269c83bc161ea6fd6b8eb03f811efa308ae902f66939efc3ad6f"
INSTRUCTION_NAMES = {"AGENTS.md", "CLAUDE.md", "GEMINI.md"}
PRIVATE_NAMES = {
    "AGENTS.override.md", "CLAUDE.local.md", "AGENTS.local.md",
    "AGENTS.md.local", "CLAUDE.md.local",
}
PROTECTED_PREFIXES = (
    "docs/sessions/", "docs/superpowers/", "docs/archive/",
    "docs/references/", ".full-review-archive/", "vendor/",
)
LEGACY_MESSAGES = {
    "missing tracked symlink to CLAUDE.md",
    "must be index mode 120000 targeting CLAUDE.md",
}


def git(repo: Path, *args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(repo), *args])


def check_instruction_index(repo: Path) -> list[str]:
    """Validate committed modes and blobs, not merely repaired working files."""
    records: dict[str, tuple[str, str]] = {}
    failures: list[str] = []
    for record in git(repo, "ls-files", "--stage", "-z").split(bytes([0])):
        if not record:
            continue
        metadata, raw_path = record.split(b"\t", 1)
        path = raw_path.decode("utf-8")
        if path.startswith(PROTECTED_PREFIXES):
            continue
        name = PurePosixPath(path).name
        if name not in INSTRUCTION_NAMES | PRIVATE_NAMES:
            continue
        mode, blob, stage = metadata.decode("ascii").split()
        if stage != "0":
            failures.append(f"{path}: unresolved instruction index entry")
            continue
        if name in PRIVATE_NAMES:
            failures.append(f"{path}: private instructions must not be tracked")
            continue
        records[path] = (mode, blob)

    directories = {PurePosixPath(".")}
    directories.update(PurePosixPath(path).parent for path in records)
    blobs: dict[str, bytes] = {}

    def content(blob: str) -> bytes:
        if blob not in blobs:
            blobs[blob] = git(repo, "cat-file", "blob", blob)
        return blobs[blob]

    for directory in sorted(directories):
        source = str(directory / "AGENTS.md")
        entry = records.get(source)
        if entry is None or entry[0] != "100644":
            failures.append(f"{source}: canonical instructions must be tracked mode 100644")
        else:
            try:
                text = content(entry[1]).decode("utf-8")
                if not text.strip():
                    failures.append(f"{source}: canonical instructions must not be empty")
            except UnicodeDecodeError:
                failures.append(f"{source}: canonical instructions must be UTF-8")
        for name in ("CLAUDE.md", "GEMINI.md"):
            alias = str(directory / name)
            entry = records.get(alias)
            if entry is None or entry[0] != "120000" or content(entry[1]) != b"AGENTS.md":
                failures.append(f"{alias}: must be tracked mode 120000 targeting exactly AGENTS.md")
    return failures


def load_fleet(path: Path) -> ModuleType:
    """Reject unknown implementations before executing the imported checker."""
    if hashlib.sha256(path.read_bytes()).hexdigest() != FLEET_SHA256:
        raise ValueError("fleet checker digest changed; review the adapter before updating its pin")
    spec = importlib.util.spec_from_file_location("labby_pinned_fleet_contract", path)
    if spec is None or spec.loader is None:
        raise ValueError("cannot load the pinned fleet contract")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def evaluate(repo: Path, fleet: ModuleType, profile: str, allow_arm64: bool) -> tuple[list[str], int]:
    # Execute the complete fleet driver first. Preserve every unrelated finding,
    # including future/unknown symlink diagnostics, instead of suppressing its
    # exit code or maintaining a partial copy of the fleet's check list.
    findings = fleet.check(repo, profile, allow_arm64=allow_arm64)
    failures: list[str] = []
    replaced = 0
    for finding in findings:
        legacy = (
            finding.check == "symlink-convention"
            and PurePosixPath(finding.path).name in {"AGENTS.md", "GEMINI.md"}
            and finding.message in LEGACY_MESSAGES
        )
        if legacy:
            replaced += 1
        else:
            failures.append(finding.render())
    failures.extend("instruction-index: " + message for message in check_instruction_index(repo))
    return failures, replaced


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--fleet-script", type=Path, required=True)
    parser.add_argument("--profile", choices=("rust", "python", "node", "go", "ops"), default="rust")
    parser.add_argument("--allow-arm64", action="store_true")
    args = parser.parse_args()
    try:
        failures, replaced = evaluate(
            args.repo.resolve(), load_fleet(args.fleet_script.resolve()), args.profile, args.allow_arm64
        )
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"repository contract failed: {error}", file=sys.stderr)
        return 1
    print(f"AGENTS-first policy: replaced {replaced} legacy direction findings with strict Git-index validation")
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        return 1
    print("repository contract valid: all fleet checks and canonical instruction index passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
