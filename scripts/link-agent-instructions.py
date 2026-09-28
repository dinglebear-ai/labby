#!/usr/bin/env python3
"""Repair AGENTS-first aliases in one Git checkout without discarding prose."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import stat
import subprocess
import sys

NAMES = {"AGENTS.md", "CLAUDE.md", "GEMINI.md"}
EXCLUDED = ("docs/sessions/", "docs/superpowers/", "docs/archive/", "docs/references/", ".full-review/", ".full-review-archive/", "vendor/")


def fingerprint(path: Path) -> tuple | None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    target = os.readlink(path) if stat.S_ISLNK(info.st_mode) else None
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, target)


def regular(path: Path) -> bool:
    return not path.is_symlink() and path.is_file()


def plan(root: Path) -> tuple[list[tuple[Path, Path]], dict[Path, tuple | None]]:
    top = subprocess.check_output(["git", "-C", str(root), "rev-parse", "--show-toplevel"], text=True).strip()
    if Path(top).resolve() != root:
        raise ValueError("target must be the Git checkout root")
    data = subprocess.check_output(["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"])
    directories = {root}
    for raw in data.split(bytes([0])):
        if not raw:
            continue
        name = os.fsdecode(raw)
        if name.startswith(EXCLUDED) or Path(name).name not in NAMES:
            continue
        directory = (root / name).parent
        if directory.resolve() != directory:
            raise ValueError(f"refusing a symlinked instruction ancestor: {name}")
        directories.add(directory)
    edits: list[tuple[Path, Path]] = []
    fingerprints: dict[Path, tuple | None] = {}
    for directory in sorted(directories):
        agents, claude = directory / "AGENTS.md", directory / "CLAUDE.md"
        if regular(agents):
            source = agents
        elif (not agents.exists() and not agents.is_symlink()) or (agents.is_symlink() and os.readlink(agents) == "CLAUDE.md"):
            if not regular(claude):
                raise ValueError(f"{directory}: no regular canonical or legacy instruction source")
            source = claude
        else:
            raise ValueError(f"{agents}: invalid canonical source")
        body = source.read_bytes()
        if not body.decode("utf-8").strip():
            raise ValueError(f"{source}: instruction source is empty")
        for name in NAMES:
            path = directory / name
            fingerprints[path] = fingerprint(path)
            if path == source or path.is_symlink() or not path.exists():
                continue
            if not regular(path) or path.read_bytes() != body:
                raise ValueError(f"{path}: conflicting content; reconcile it before relinking")
        edits.append((directory, source))
    return edits, fingerprints


def repair(root: Path) -> int:
    edits, before = plan(root)
    # Preflight every scope before touching any of them. Detect concurrent edits
    # between validation and application; this is not a cross-process lock.
    if any(fingerprint(path) != expected for path, expected in before.items()):
        raise ValueError("instruction files changed during preflight; retry after reconciliation")
    for directory, source in edits:
        agents = directory / "AGENTS.md"
        if source != agents:
            os.replace(source, agents)
        for name in ("CLAUDE.md", "GEMINI.md"):
            alias = directory / name
            if alias.is_symlink() and os.readlink(alias) == "AGENTS.md":
                continue
            if alias.exists() or alias.is_symlink():
                alias.unlink()
            alias.symlink_to("AGENTS.md")
    return len(edits)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", type=Path, default=Path.cwd())
    args = parser.parse_args()
    try:
        count = repair(args.root.resolve(strict=True))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"instruction relink failed: {error}", file=sys.stderr)
        return 1
    print(f"validated {count} AGENTS-first scopes; no files staged")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
