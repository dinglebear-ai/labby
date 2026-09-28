#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "check-product-docs.py"
SPEC = importlib.util.spec_from_file_location("check_product_docs", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProductDocsServiceIndexTests(unittest.TestCase):
    def test_only_first_table_column_counts_as_service_index_coverage(self) -> None:
        text = (
            "`proxy` mentioned in prose must not count.\n\n"
            "| Service | Documentation |\n"
            "| --- | --- |\n"
            "| `access`, `projects` | [Access](./ACCESS.md) |\n"
            "| gateway runtime | Mentions `gateway` only in the second column |\n"
        )
        self.assertEqual(MODULE.service_names_from_table(text), {"access", "projects"})

    def test_named_section_selects_its_own_service_table(self) -> None:
        text = (
            "| `wrong` | Earlier table |\n"
            "| --- | --- |\n\n"
            "## Current Product Services\n\n"
            "| Service | Product doc |\n"
            "| --- | --- |\n"
            "| `gateway`, `proxy` | Docs |\n"
        )
        self.assertEqual(
            MODULE.service_names_from_table(text, "Current Product Services"),
            {"gateway", "proxy"},
        )
        self.assertEqual(MODULE.service_names_from_table(text, "Missing"), set())


class ProductDocsInstructionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.root_patch = patch.object(MODULE, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.make_scope(self.root)

    def make_scope(self, directory: Path) -> None:
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "AGENTS.md").write_text(f"# Instructions for {directory.name}\n", encoding="utf-8")
        for name in ("CLAUDE.md", "GEMINI.md"):
            (directory / name).symlink_to("AGENTS.md")

    def failures(self) -> list[str]:
        failures: list[str] = []
        MODULE.validate_instruction_symlinks(failures)
        return failures

    def test_valid_root_and_nested_scopes(self) -> None:
        self.make_scope(self.root / "crates" / "example")
        self.assertEqual(self.failures(), [])

    def test_reversed_topology_is_rejected(self) -> None:
        (self.root / "CLAUDE.md").unlink()
        (self.root / "AGENTS.md").rename(self.root / "CLAUDE.md")
        (self.root / "AGENTS.md").symlink_to("CLAUDE.md")
        self.assertTrue(any("regular file" in failure for failure in self.failures()))

    def test_missing_canonical_with_orphan_alias_is_rejected(self) -> None:
        self.make_scope(self.root / "nested")
        (self.root / "nested" / "AGENTS.md").unlink()
        self.assertTrue(any("nested/AGENTS.md" in failure for failure in self.failures()))

    def test_root_is_required_even_without_any_instruction_files(self) -> None:
        for name in ("AGENTS.md", "CLAUDE.md", "GEMINI.md"):
            (self.root / name).unlink()
        self.assertEqual(len(self.failures()), 3)

    def test_empty_canonical_is_rejected(self) -> None:
        (self.root / "AGENTS.md").write_text(" \n", encoding="utf-8")
        self.assertTrue(any("must not be empty" in failure for failure in self.failures()))

    def test_copied_alias_is_rejected(self) -> None:
        alias = self.root / "CLAUDE.md"
        alias.unlink()
        alias.write_text("# Divergent copy\n", encoding="utf-8")
        self.assertTrue(any("CLAUDE.md: missing symlink" in failure for failure in self.failures()))

    def test_wrong_and_absolute_alias_targets_are_rejected(self) -> None:
        alias = self.root / "GEMINI.md"
        for target in ("CLAUDE.md", str(self.root / "AGENTS.md"), "./AGENTS.md", "missing.md"):
            with self.subTest(target=target):
                alias.unlink()
                alias.symlink_to(target)
                self.assertTrue(any("expected symlink target AGENTS.md" in failure for failure in self.failures()))

    def test_ignored_worktrees_and_private_overrides_are_not_audited(self) -> None:
        (self.root / ".gitignore").write_text(".worktrees/\ntarget/\nAGENTS.override.md\n", encoding="utf-8")
        for prefix in (".worktrees/other", "target/build"):
            directory = self.root / prefix
            directory.mkdir(parents=True)
            (directory / "CLAUDE.md").write_text("# Other checkout\n", encoding="utf-8")
        (self.root / "AGENTS.override.md").write_text("private", encoding="utf-8")
        self.assertEqual(self.failures(), [])
        self.assertEqual(set(MODULE.canonical_docs()), {self.root / "AGENTS.md"})
        self.assertFalse(any(p.name == "AGENTS.override.md" for p in MODULE.repository_paths()))

    def test_tracked_private_instructions_are_rejected_even_when_ignored(self) -> None:
        names = ("AGENTS.override.md", "CLAUDE.local.md", "AGENTS.local.md", "AGENTS.md.local", "CLAUDE.md.local")
        (self.root / ".gitignore").write_text("\n".join(names) + "\n", encoding="utf-8")
        for name in names:
            (self.root / name).write_text("private fixture", encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "add", "-f", "--", *names], check=True)
        self.assertEqual(len(self.failures()), len(names))
        self.assertTrue(all("private instructions must remain" in failure for failure in self.failures()))

    def test_unignored_private_instructions_are_rejected_before_staging(self) -> None:
        (self.root / "AGENTS.override.md").write_text("private fixture", encoding="utf-8")
        self.assertTrue(any("private instructions must remain" in failure for failure in self.failures()))

    def test_protected_history_is_excluded_even_when_tracked(self) -> None:
        for prefix in ("docs/sessions", "docs/superpowers", "docs/archive"):
            directory = self.root / prefix
            directory.mkdir(parents=True)
            (directory / "CLAUDE.md").write_text("# Historical\n[old](missing.md)\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "add", "docs"], check=True)
        self.assertEqual(self.failures(), [])
        self.assertFalse(any("docs" in p.relative_to(self.root).parts for p in MODULE.repository_paths()))

    def test_canonical_inventory_contains_agents_once_not_aliases(self) -> None:
        subprocess.run(["git", "-C", str(self.root), "add", "AGENTS.md", "CLAUDE.md", "GEMINI.md"], check=True)
        self.assertEqual(MODULE.canonical_docs(), [self.root / "AGENTS.md"])

    def test_untracked_new_canonical_scope_is_checked(self) -> None:
        directory = self.root / "new-scope"
        directory.mkdir()
        (directory / "AGENTS.md").write_text("# New\n", encoding="utf-8")
        self.assertEqual(len(self.failures()), 2)
        self.assertIn(directory / "AGENTS.md", MODULE.canonical_docs())

    def test_broken_canonical_instruction_link_is_checked(self) -> None:
        agents = self.root / "AGENTS.md"
        agents.write_text("# Guide\n[broken](missing.md)\n", encoding="utf-8")
        failures: list[str] = []
        for path in MODULE.canonical_docs():
            MODULE.validate_links(path, failures)
        self.assertTrue(any("missing local link target" in failure for failure in failures))

    def budget_failures(self) -> list[str]:
        failures: list[str] = []
        MODULE.validate_instruction_budgets(failures)
        return failures

    def test_root_character_budget_boundary(self) -> None:
        root = self.root / "AGENTS.md"
        root.write_text("x" * 7900)
        self.assertEqual(self.budget_failures(), [])
        root.write_text("x" * 7901)
        self.assertTrue(any("7900-character" in item for item in self.budget_failures()))

    def test_root_character_count_is_not_utf8_byte_count(self) -> None:
        (self.root / "AGENTS.md").write_text("é" * 7900, encoding="utf-8")
        self.assertEqual(self.budget_failures(), [])

    def test_nested_budget_counts_all_ancestors_and_separators(self) -> None:
        self.make_scope(self.root / "one")
        self.make_scope(self.root / "one/two")
        (self.root / "AGENTS.md").write_text("x" * 7900)
        (self.root / "one/AGENTS.md").write_text("x" * 10000)
        leaf = self.root / "one/two/AGENTS.md"
        leaf.write_text("x" * (32768 - 7900 - 10000 - 4))
        self.assertEqual(self.budget_failures(), [])
        leaf.write_text(leaf.read_text() + "x")
        self.assertTrue(any("one/two/AGENTS.md" in item for item in self.budget_failures()))

    def test_nested_budget_uses_utf8_bytes(self) -> None:
        self.make_scope(self.root / "unicode")
        (self.root / "unicode/AGENTS.md").write_text("é" * 17000, encoding="utf-8")
        self.assertTrue(any("UTF-8 bytes" in item for item in self.budget_failures()))

    def test_sibling_instructions_are_not_combined(self) -> None:
        for name in ("one", "two"):
            self.make_scope(self.root / name)
            (self.root / name / "AGENTS.md").write_text("x" * 25000)
        self.assertEqual(self.budget_failures(), [])

    def test_budget_excludes_ignored_private_and_protected_files(self) -> None:
        (self.root / ".gitignore").write_text(".worktrees/\nAGENTS.override.md\n")
        for name in (".worktrees/other", "docs/sessions"):
            self.make_scope(self.root / name)
            (self.root / name / "AGENTS.md").write_bytes(bytes([255]))
        (self.root / "AGENTS.override.md").write_bytes(bytes([255]))
        self.assertEqual(self.budget_failures(), [])

    def test_invalid_utf8_has_an_actionable_failure(self) -> None:
        (self.root / "AGENTS.md").write_bytes(bytes([255]))
        self.assertTrue(any("must be UTF-8" in item for item in self.budget_failures()))

    def test_git_inventory_failure_is_not_silently_ignored(self) -> None:
        with patch.object(MODULE.subprocess, "check_output", side_effect=subprocess.CalledProcessError(128, "git")):
            with self.assertRaises(subprocess.CalledProcessError):
                MODULE.repository_paths()


if __name__ == "__main__":
    unittest.main()
