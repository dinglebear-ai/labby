#!/usr/bin/env python3
"""Regression tests for the complete fleet driver and AGENTS-first index policy."""

from __future__ import annotations

from dataclasses import dataclass
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

SCRIPT = Path(__file__).with_name("check_repository_contract.py")
SPEC = importlib.util.spec_from_file_location("repository_contract_adapter", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


@dataclass
class Finding:
    check: str
    path: str
    message: str

    def render(self) -> str:
        return f"{self.check}: {self.path}: {self.message}"


class InstructionIndexTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git("init", "-q")
        self.scope(self.root)
        self.git("add", ".")

    def git(self, *args: str) -> str:
        return subprocess.check_output(["git", "-C", str(self.root), *args], text=True)

    def scope(self, directory: Path) -> None:
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "AGENTS.md").write_text("# Canonical instructions\n")
        for name in ("CLAUDE.md", "GEMINI.md"):
            (directory / name).symlink_to("AGENTS.md")

    def check(self) -> list[str]:
        return MODULE.check_instruction_index(self.root)

    def test_root_and_nested_index(self) -> None:
        self.scope(self.root / "nested")
        self.git("add", ".")
        self.assertEqual(self.check(), [])

    def test_regular_alias_is_rejected(self) -> None:
        alias = self.root / "CLAUDE.md"
        alias.unlink()
        alias.write_text("AGENTS.md")
        self.git("add", ".")
        self.assertTrue(any("CLAUDE.md: must be tracked mode 120000" in item for item in self.check()))

    def test_repaired_working_file_does_not_hide_invalid_index(self) -> None:
        alias = self.root / "GEMINI.md"
        alias.unlink()
        alias.symlink_to("CLAUDE.md")
        self.git("add", ".")
        alias.unlink()
        alias.symlink_to("AGENTS.md")
        self.assertTrue(any("GEMINI.md" in item for item in self.check()))

    def test_absolute_target_is_rejected(self) -> None:
        alias = self.root / "CLAUDE.md"
        alias.unlink()
        alias.symlink_to(self.root / "AGENTS.md")
        self.git("add", ".")
        self.assertTrue(self.check())

    def test_missing_source_and_root_are_rejected(self) -> None:
        self.git("rm", "--cached", "AGENTS.md")
        self.assertTrue(any("AGENTS.md: canonical" in item for item in self.check()))
        self.git("rm", "--cached", "CLAUDE.md", "GEMINI.md")
        self.assertEqual(len(self.check()), 3)

    def test_reversed_topology_is_rejected(self) -> None:
        (self.root / "CLAUDE.md").unlink()
        (self.root / "AGENTS.md").rename(self.root / "CLAUDE.md")
        (self.root / "AGENTS.md").symlink_to("CLAUDE.md")
        self.git("add", ".")
        self.assertTrue(any("canonical instructions must be tracked" in item for item in self.check()))

    def test_empty_source_is_rejected(self) -> None:
        (self.root / "AGENTS.md").write_text(" \n")
        self.git("add", ".")
        self.assertTrue(any("must not be empty" in item for item in self.check()))

    def test_non_utf8_source_is_rejected(self) -> None:
        (self.root / "AGENTS.md").write_bytes(bytes([255]))
        self.git("add", ".")
        self.assertTrue(any("must be UTF-8" in item for item in self.check()))

    def test_private_override_must_not_be_tracked(self) -> None:
        (self.root / ".gitignore").write_text("AGENTS.override.md\n")
        (self.root / "AGENTS.override.md").write_text("private fixture")
        self.git("add", "-f", "AGENTS.override.md")
        self.assertTrue(any("private instructions must not be tracked" in item for item in self.check()))

    def test_protected_history_is_not_read(self) -> None:
        self.scope(self.root / "docs/sessions/old")
        (self.root / "docs/sessions/old/AGENTS.md").write_bytes(bytes([255]))
        self.git("add", ".")
        self.assertEqual(self.check(), [])

    def test_unresolved_index_is_rejected(self) -> None:
        record = b"100644 " + b"1" * 40 + b" 2\tAGENTS.md" + bytes([0])
        with patch.object(MODULE, "git", return_value=record):
            self.assertTrue(any("unresolved instruction index" in item for item in self.check()))

    def test_other_fleet_failures_are_not_suppressed(self) -> None:
        legacy = Finding("symlink-convention", "AGENTS.md", "must be index mode 120000 targeting CLAUDE.md")
        other = Finding("docs-frontmatter", "docs/dev/example.md", "missing keys: title")
        fleet = Mock()
        fleet.check.return_value = [legacy, other]
        failures, replaced = MODULE.evaluate(self.root, fleet, "rust", True)
        self.assertEqual(failures, [other.render()])
        self.assertEqual(replaced, 1)
        fleet.check.assert_called_once_with(self.root, "rust", allow_arm64=True)

    def test_unknown_symlink_diagnostic_is_not_suppressed(self) -> None:
        finding = Finding("symlink-convention", "AGENTS.md", "new upstream requirement")
        fleet = Mock()
        fleet.check.return_value = [finding]
        self.assertEqual(MODULE.evaluate(self.root, fleet, "rust", False), ([finding.render()], 0))

    def test_no_legacy_findings_still_validates_instruction_index(self) -> None:
        self.git("rm", "--cached", "GEMINI.md")
        fleet = Mock()
        fleet.check.return_value = []
        failures, replaced = MODULE.evaluate(self.root, fleet, "rust", False)
        self.assertTrue(failures)
        self.assertEqual(replaced, 0)

    def test_unknown_checker_is_rejected_before_execution(self) -> None:
        script = self.root / "unexpected.py"
        script.write_text("raise RuntimeError('must never execute')\n")
        with self.assertRaisesRegex(ValueError, "digest changed"):
            MODULE.load_fleet(script)

    def test_fleet_exception_is_not_converted_to_success(self) -> None:
        fleet = Mock()
        fleet.check.side_effect = RuntimeError("checker failed")
        with self.assertRaisesRegex(RuntimeError, "checker failed"):
            MODULE.evaluate(self.root, fleet, "rust", False)


if __name__ == "__main__":
    unittest.main()
