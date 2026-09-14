"""Regression tests for the MCP specification Just recipes."""

from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci.test_extract_mcp_schema_requirements import ROOT, pinned_source


PINNED_REVISION = "5f5440bb26a62e2cf3440b92da5a667efa03b267"
EXPECTED_ORIGIN = "https://github.com/modelcontextprotocol/modelcontextprotocol"


class McpSpecRecipeTests(unittest.TestCase):
    def test_extractor_source_override_resolves_from_repository_root(self) -> None:
        with patch.dict(os.environ, {"LABBY_MCP_SPEC_SOURCE": "tmp/spec"}):
            self.assertEqual(pinned_source(), ROOT / "tmp/spec")

    def test_check_passes_custom_source_to_tests_without_default_preparation(self) -> None:
        justfile = (ROOT / "Justfile").read_text()
        start = justfile.index("mcp-spec-check spec_checkout=")
        end = justfile.index("\n# Execute all registered oracles", start)
        recipe = justfile[start:end]
        self.assertIn("LABBY_MCP_SPEC_SOURCE={{quote(spec_checkout)}}", recipe)
        self.assertNotIn("just mcp-spec-source", recipe)

    def test_existing_empty_no_checkout_clone_can_be_materialized(self) -> None:
        source = pinned_source()
        self.assertTrue((source / ".git").exists())
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "spec"
            subprocess.run(
                ["git", "clone", "--quiet", "--no-checkout", str(source), str(checkout)],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(checkout), "remote", "set-url", "origin", EXPECTED_ORIGIN],
                check=True,
            )
            self.assertFalse((checkout / ".git/index").exists())
            subprocess.run(
                ["just", "mcp-spec-source", str(checkout)], cwd=ROOT, check=True,
                stdout=subprocess.DEVNULL,
            )
            revision = subprocess.check_output(
                ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True,
            ).strip()
            self.assertEqual(revision, PINNED_REVISION)
            status = subprocess.check_output(
                ["git", "-C", str(checkout), "status", "--porcelain=v1", "--untracked-files=all"],
                text=True,
            )
            self.assertEqual(status, "")

    def test_no_checkout_clone_with_worktree_content_is_rejected(self) -> None:
        source = pinned_source()
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "spec"
            subprocess.run(
                ["git", "clone", "--quiet", "--no-checkout", str(source), str(checkout)],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(checkout), "remote", "set-url", "origin", EXPECTED_ORIGIN],
                check=True,
            )
            (checkout / "operator-note").write_text("preserve me\n")
            completed = subprocess.run(
                ["just", "mcp-spec-source", str(checkout)], cwd=ROOT,
                capture_output=True, text=True,
            )
            self.assertEqual(completed.returncode, 2)
            self.assertIn("refusing to modify dirty MCP source checkout", completed.stderr)
            self.assertEqual((checkout / "operator-note").read_text(), "preserve me\n")


if __name__ == "__main__":
    unittest.main()
