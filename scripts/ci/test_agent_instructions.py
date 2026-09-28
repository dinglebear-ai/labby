#!/usr/bin/env python3
"""Exercise the legacy entrypoint against isolated Git repositories."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "plugins/scripts/link-claude-mds"
SPEC = importlib.util.spec_from_file_location("link_instructions", ROOT / "scripts/link-agent-instructions.py")
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class InstructionLinkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "repository with spaces"
        self.root.mkdir()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        (self.root / "AGENTS.md").write_text("# Root instructions\n")

    def run_helper(self):
        return subprocess.run(["bash", str(ENTRY), str(self.root)], capture_output=True, text=True)

    def assert_scope(self, directory):
        self.assertTrue((directory / "AGENTS.md").is_file())
        self.assertFalse((directory / "AGENTS.md").is_symlink())
        for name in ("CLAUDE.md", "GEMINI.md"):
            self.assertEqual((directory / name).readlink(), Path("AGENTS.md"))

    def test_root_nested_and_idempotent(self):
        nested = self.root / "nested with spaces"
        nested.mkdir()
        (nested / "AGENTS.md").write_text("# Nested\n")
        self.assertEqual(self.run_helper().returncode, 0)
        self.assert_scope(self.root)
        self.assert_scope(nested)
        before = (self.root / "AGENTS.md").stat().st_ino
        self.assertEqual(self.run_helper().returncode, 0)
        self.assertEqual((self.root / "AGENTS.md").stat().st_ino, before)
        staged = subprocess.check_output(["git", "-C", str(self.root), "diff", "--cached", "--name-only"])
        self.assertEqual(staged, b"")

    def test_legacy_canonical_is_promoted_without_losing_text(self):
        (self.root / "AGENTS.md").rename(self.root / "CLAUDE.md")
        (self.root / "AGENTS.md").symlink_to("CLAUDE.md")
        (self.root / "GEMINI.md").symlink_to("CLAUDE.md")
        self.assertEqual(self.run_helper().returncode, 0)
        self.assert_scope(self.root)
        self.assertEqual((self.root / "AGENTS.md").read_text(), "# Root instructions\n")

    def test_claude_only_legacy_scope_is_promoted(self):
        (self.root / "AGENTS.md").rename(self.root / "CLAUDE.md")
        self.assertEqual(self.run_helper().returncode, 0)
        self.assert_scope(self.root)

    def test_identical_alias_copy_can_be_replaced(self):
        (self.root / "CLAUDE.md").write_bytes((self.root / "AGENTS.md").read_bytes())
        self.assertEqual(self.run_helper().returncode, 0)
        self.assert_scope(self.root)

    def test_conflict_fails_before_any_scope_changes(self):
        nested = self.root / "z-conflict"
        nested.mkdir()
        (nested / "AGENTS.md").write_text("canonical")
        (nested / "CLAUDE.md").write_text("unique legacy notes")
        result = self.run_helper()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("conflicting content", result.stderr)
        self.assertFalse((self.root / "CLAUDE.md").exists())
        self.assertEqual((nested / "CLAUDE.md").read_text(), "unique legacy notes")

    def test_wrong_alias_is_repaired_but_wrong_source_is_not(self):
        (self.root / "GEMINI.md").symlink_to("missing")
        self.assertEqual(self.run_helper().returncode, 0)
        self.assert_scope(self.root)
        (self.root / "AGENTS.md").unlink()
        (self.root / "AGENTS.md").symlink_to("missing")
        self.assertNotEqual(self.run_helper().returncode, 0)

    def test_empty_or_invalid_utf8_source_fails(self):
        for value in (b" \n", bytes([255])):
            with self.subTest(value=value):
                (self.root / "AGENTS.md").write_bytes(value)
                self.assertNotEqual(self.run_helper().returncode, 0)

    def test_ignored_worktree_and_private_files_are_untouched(self):
        (self.root / ".gitignore").write_text(".worktrees/\nAGENTS.override.md\nCLAUDE.local.md\n")
        directory = self.root / ".worktrees/other"
        directory.mkdir(parents=True)
        (directory / "CLAUDE.md").write_text("other checkout")
        (self.root / "AGENTS.override.md").write_text("private")
        (self.root / "CLAUDE.local.md").symlink_to("AGENTS.override.md")
        self.assertEqual(self.run_helper().returncode, 0)
        self.assertFalse((directory / "AGENTS.md").exists())
        self.assertEqual((self.root / "CLAUDE.local.md").readlink(), Path("AGENTS.override.md"))
        self.assertEqual((self.root / "AGENTS.override.md").read_text(), "private")

    def test_tracked_protected_history_is_not_relinked(self):
        for prefix in ("docs/sessions", "docs/superpowers", "docs/archive", "vendor"):
            directory = self.root / prefix
            directory.mkdir(parents=True)
            (directory / "CLAUDE.md").write_bytes(bytes([255]))
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        self.assertEqual(self.run_helper().returncode, 0)
        self.assertFalse((self.root / "docs/sessions/AGENTS.md").exists())

    def test_nongit_target_fails(self):
        other = self.root.parent / "nongit"
        other.mkdir()
        result = subprocess.run(["bash", str(ENTRY), str(other)], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_concurrent_change_aborts_before_application(self):
        with patch.object(MODULE, "fingerprint", side_effect=[None, None, None, ("changed",)]):
            with self.assertRaisesRegex(ValueError, "changed during preflight"):
                MODULE.repair(self.root)
        self.assertFalse((self.root / "CLAUDE.md").exists())


if __name__ == "__main__":
    unittest.main()
