#!/usr/bin/env python3
"""Materialize self-contained client packages from the canonical Labby skills."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "plugins/labby"
SOURCE = PACKAGE / ".apm/skills"
INSTALL_SKILL = ROOT / "plugins/install-labby/skills/install-labby"
TARGETS = (
    "claude", "codex", "copilot", "cursor", "gemini", "antigravity",
    "opencode", "windsurf", "kiro", "hermes", "grok-build",
    "intellij", "agent-skills",
)
EXPERIMENTAL = ("copilot-cowork", "copilot-app", "grok-cloud", "openclaw")
NO_MCP = {"grok-build", "agent-skills", "grok-cloud", "openclaw"}
NATIVE_SKILL_PATH = {
    "grok-cloud": ".grok/skills",
    "openclaw": ".agents/skills",
}


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def build(target: str, output: Path) -> None:
    """Build the portable package, then retain APM's native target projection."""
    output.mkdir(parents=True)
    shutil.copytree(SOURCE, output / "skills")
    shutil.copy2(ROOT / "LICENSE", output / "LICENSE")
    manifest = {
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": "labby",
        "description": "Labby gateway skills and MCP client integration.",
        "author": {"name": "dinglebear-ai"},
        "repository": "https://github.com/dinglebear-ai/labby",
        "license": "AGPL-3.0-only",
    }
    write_json(output / "plugin.json", manifest)
    if target not in NO_MCP:
        write_json(output / "mcp.json", {
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {"labby": {
                "type": "stdio", "command": "npx",
                "args": ["-y", "@dinglebear/labby", "mcp"],
            }},
        })

    if target == "claude":
        claude = json.loads((PACKAGE / ".claude-plugin/plugin.json").read_text())
        claude["skills"] = "./skills"
        claude["mcpServers"] = "./.mcp.json"
        write_json(output / ".claude-plugin/plugin.json", claude)
        write_json(output / ".mcp.json", {"mcpServers": json.loads((output / "mcp.json").read_text())["mcpServers"]})

    (output / "README.md").write_text(
        f"# Labby for {target}\n\n"
        "This directory is a self-contained Labby Agent Plugin. Install this "
        "directory with a client that supports Agent Plugins 1.0. "
        "It includes the five Labby skills and, where supported, a stdio MCP "
        "connection through `npx -y @dinglebear/labby mcp`.\n\n"
        "The client-native files in hidden directories are provided for "
        "clients that load skills and MCP settings directly. Copy the skill "
        "folders to that client's documented skills location and merge MCP "
        "settings with existing settings; do not overwrite existing config.\n\n"
        "Generated from `plugins/labby/.apm/skills` by "
        "`python3 scripts/generate-native-plugins.py`.\n"
    )

    if target in EXPERIMENTAL:
        if target in NATIVE_SKILL_PATH:
            shutil.copytree(SOURCE, output / NATIVE_SKILL_PATH[target])
        return

    projection_target = "copilot" if target == "intellij" else target
    with tempfile.TemporaryDirectory(prefix=f"labby-{target}-") as scratch:
        source = Path(scratch) / "source"
        source.mkdir()
        shutil.copytree(SOURCE, source / ".apm/skills")
        mcp = "" if target in NO_MCP else (
            "  mcp:\n    - name: labby\n      registry: false\n"
            "      transport: stdio\n      command: npx\n"
            "      args: ['-y', '@dinglebear/labby', 'mcp']\n"
        )
        (source / "apm.yml").write_text(
            f"name: labby\nversion: 0.1.0\ntargets: [{projection_target}]\n"
            f"includes: auto\ndependencies:\n  apm: []\n{mcp}"
        )
        workspace = Path(scratch) / "consumer"
        workspace.mkdir()
        (workspace / "apm.yml").write_text(
            f"name: labby-native-preview\nversion: 0.1.0\n"
            f"targets: [{projection_target}]\ndependencies:\n  apm:\n    - {source}\n"
        )
        result = subprocess.run(
            ["apm", "install", "--target", projection_target], cwd=workspace,
            capture_output=True, text=True, timeout=120,
        )
        if result.returncode:
            raise RuntimeError(f"APM projection failed for {target}:\n{result.stdout}\n{result.stderr}")
        for native in (".agents", ".claude", ".codex", ".github", ".cursor",
                       ".gemini", ".opencode", ".windsurf", ".kiro", ".grok"):
            source = workspace / native
            if source.is_dir():
                shutil.copytree(source, output / native, dirs_exist_ok=True)
        for native in ("opencode.json", "config.yaml", "mcp_config.json", ".mcp.json"):
            source_file = workspace / native
            destination = output / native
            if source_file.is_file() and not destination.exists():
                shutil.copy2(source_file, destination)


def tree_files(path: Path) -> dict[str, bytes]:
    return {str(p.relative_to(path)): p.read_bytes() for p in path.rglob("*") if p.is_file()}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="labby-native-plugins-") as scratch:
        staged = Path(scratch)
        for target in (*TARGETS, *EXPERIMENTAL):
            build(target, staged / target)
        if args.check:
            actual = {name: tree_files(PACKAGE / name) for name in (*TARGETS, *EXPERIMENTAL)}
            expected = {name: tree_files(staged / name) for name in (*TARGETS, *EXPERIMENTAL)}
            stale = [name for name in expected if actual[name] != expected[name]]
            if tree_files(INSTALL_SKILL) != tree_files(SOURCE / "install-labby"):
                stale.append("install-labby")
            if stale:
                print("stale native packages: " + ", ".join(stale))
                return 1
            print("native packages match canonical skills and APM projections")
            return 0
        for target in (*TARGETS, *EXPERIMENTAL):
            dest = PACKAGE / target
            if dest.exists():
                shutil.rmtree(dest)
            shutil.copytree(staged / target, dest)
            print(f"generated {dest.relative_to(ROOT)}")
        if INSTALL_SKILL.exists():
            shutil.rmtree(INSTALL_SKILL)
        shutil.copytree(SOURCE / "install-labby", INSTALL_SKILL)
        print(f"generated {INSTALL_SKILL.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
