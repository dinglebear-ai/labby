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


if __name__ == "__main__":
    unittest.main()
