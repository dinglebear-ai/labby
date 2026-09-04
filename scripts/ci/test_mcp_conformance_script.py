import subprocess
import unittest
import tomllib

from scripts.ci.check_mcp_sdk_pin import matches_pin
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class McpConformanceScriptTests(unittest.TestCase):
    def test_sdk_pin_is_semantic_not_dependent_on_toml_key_order(self):
        for declaration in [
            'rmcp = { git = "repo", rev = "revision", version = "=3.1.4" }',
            'rmcp = { version = "=3.1.4", rev = "revision", git = "repo" }',
            '[workspace.dependencies.rmcp]\nversion = "=3.1.4"\nrev = "revision"\ngit = "repo"',
        ]:
            with self.subTest(declaration=declaration):
                manifest = tomllib.loads('[workspace.dependencies]\n' + declaration)
                self.assertTrue(matches_pin(manifest, "repo", "revision"))

    def test_sdk_pin_rejects_wrong_source_or_revision_and_inactive_text(self):
        for declaration in [
            'rmcp = { git = "wrong", rev = "revision" }',
            'rmcp = { git = "repo", rev = "wrong" }',
            'rmcp = { git = "repo", branch = "revision" }',
            'rmcp = { git = "repo", rev = "revision", tag = "mutable" }',
            '# rmcp = { git = "repo", rev = "revision" }',
            'rmcp = "=3.1.4"',
        ]:
            with self.subTest(declaration=declaration):
                manifest = tomllib.loads('[workspace.dependencies]\n' + declaration)
                self.assertFalse(matches_pin(manifest, "repo", "revision"))

    def test_help_advertises_direct_proxy_only_mode(self) -> None:
        completed = subprocess.run(
            ["bash", str(ROOT / "scripts/ci/mcp-conformance.sh"), "--help"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=3,
        )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("--direct-proxy-only", completed.stdout)
        self.assertIn("MCP_CONFORMANCE_OUTPUT_DIR", completed.stdout)

    def test_script_honors_cargo_target_dir_and_absolute_output(self) -> None:
        script = (ROOT / "scripts/ci/mcp-conformance.sh").read_text()

        self.assertIn(
            'cargo_target_dir="${CARGO_TARGET_DIR:-${repo_root}/target}"', script
        )
        self.assertIn(
            'rmcp_target_dir="${CARGO_TARGET_DIR:-${work_dir}/rust-sdk/target}"',
            script,
        )
        self.assertIn('"${rmcp_target_dir}/debug/conformance-server"', script)
        self.assertIn('"${rmcp_target_dir}/debug/conformance-client"', script)
        self.assertIn('"${cargo_target_dir}/debug/labby" --json proxy', script)
        self.assertIn('"${cargo_target_dir}/debug/stdio-mcp-fixture"', script)
        self.assertIn('"${cargo_target_dir}/debug/labby" serve', script)
        self.assertIn('if [[ "$MCP_CONFORMANCE_OUTPUT_DIR" = /* ]]', script)


if __name__ == "__main__":
    unittest.main()
