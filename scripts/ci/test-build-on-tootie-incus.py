#!/usr/bin/env python3
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "build-on-tootie-incus.sh"


class TootieIncusBuildScriptTest(unittest.TestCase):
    def run_script(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_help_documents_isolation_and_export(self) -> None:
        result = self.run_script("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("never starts, stops, restarts", result.stdout)
        self.assertIn("docker save", result.stdout)
        self.assertIn("--preflight-only", result.stdout)

    def test_revision_is_required(self) -> None:
        result = self.run_script("--base-image", "sha256:" + "a" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("REVISION is required", result.stderr)

    def test_base_image_must_be_immutable(self) -> None:
        result = self.run_script("HEAD", "--base-image", "labby:latest")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("digest-pinned", result.stderr)

    def test_script_discovers_the_live_incus_socket_without_initializing(self) -> None:
        text = SCRIPT.read_text()
        self.assertIn("/mnt/cache/incus/state", text)
        self.assertIn("INCUS_DIR", text)
        self.assertNotIn("incus admin init", text)
        self.assertNotIn("incus init", text)

    def test_script_builds_as_unprivileged_user_and_never_controls_service(self) -> None:
        text = SCRIPT.read_text()
        self.assertIn("su -s /bin/bash - labby", text)
        self.assertNotIn("systemctl restart", text)
        self.assertNotIn("incus restart", text)
        self.assertNotIn("incus stop", text)


if __name__ == "__main__":
    unittest.main()
