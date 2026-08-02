import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class McpConformanceScriptTests(unittest.TestCase):
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
        self.assertIn('"${cargo_target_dir}/debug/labby" --json proxy', script)
        self.assertIn('"${cargo_target_dir}/debug/stdio-mcp-fixture"', script)
        self.assertIn('"${cargo_target_dir}/debug/labby" serve', script)
        self.assertIn('if [[ "$MCP_CONFORMANCE_OUTPUT_DIR" = /* ]]', script)


if __name__ == "__main__":
    unittest.main()
