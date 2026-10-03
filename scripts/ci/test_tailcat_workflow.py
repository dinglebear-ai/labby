"""Keep deterministic Tailcat safety suites routed and required without live assets."""
from pathlib import Path
import os
import re
import subprocess
import unittest

import yaml

from scripts.ci.changed_paths import classify

ROOT = Path(__file__).resolve().parents[2]


class TailcatWorkflowTests(unittest.TestCase):
    def test_isolated_npm_version_helper_routes_its_release_contracts(self) -> None:
        routed = classify("pull_request", ["scripts/sync-npm-release-version.py"])
        self.assertTrue(routed["workflow"], "installer contracts exercise npm companion versions")
        self.assertTrue(routed["release"], "release hardening exercises future version bumps")

    @classmethod
    def setUpClass(cls):
        cls.ci = yaml.load((ROOT / ".github/workflows/ci.yml").read_text(), Loader=yaml.BaseLoader)

    def test_every_isolated_tailcat_input_routes_qualification(self):
        for path in [
            "packages/labby-tailcat-browser/transport.mjs",
            "packages/labby-tailcat-browser/package-lock.json",
            "packages/labby-microsandbox/cleanup.mjs",
            "packages/labby-microsandbox/package-lock.json",
            "tools/tailcat-bridge/server.go",
            "tools/tailcat-bridge/web/main_js.go",
            "tools/tailcat-bridge/go.mod",
            "tools/tailcat-bridge/go.sum",
            "scripts/test-tailcat-bridge.sh",
            "scripts/build-tailcat-bridge.sh",
            "scripts/package-tailcat-release.sh",
            "scripts/install-tailcat-depot-assets.sh",
            "scripts/ci/test_tailcat_workflow.py",
        ]:
            with self.subTest(path=path):
                self.assertTrue(classify("pull_request", [path]).get("tailcat", False))
        self.assertFalse(classify("pull_request", ["docs/runtime/CONFIG.md"]).get("tailcat", False))
        for event in ["workflow_dispatch", "schedule"]:
            self.assertTrue(classify(event, []).get("tailcat", False))

    def test_hosted_lane_is_bounded_credentialless_and_runs_real_suites(self):
        job = self.ci["jobs"].get("tailcat-tests")
        self.assertIsNotNone(job, "Tailcat safety suites need their own PR job")
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertEqual(job["needs"], "changes")
        self.assertEqual(job["timeout-minutes"], "20")
        self.assertIn("needs.changes.outputs.tailcat == 'true'", job["if"])
        self.assertEqual(self.ci["permissions"], {"contents": "read"})
        steps = job["steps"]
        self.assertEqual(steps[0]["with"]["persist-credentials"], "false")
        node = next(s for s in steps if s.get("uses", "").startswith("actions/setup-node@"))
        go = next(s for s in steps if s.get("uses", "").startswith("actions/setup-go@"))
        self.assertEqual(node["with"]["node-version"], "24")
        self.assertEqual(go["with"]["go-version"], "1.27.1")
        runs = "\n".join(s.get("run", "") for s in steps)
        for package in ["labby-tailcat-browser", "labby-microsandbox"]:
            self.assertIn(f"npm ci --prefix packages/{package} --ignore-scripts --no-audit --no-fund", runs)
            self.assertIn(f"npm test --prefix packages/{package}", runs)
        self.assertIn("go mod verify", runs)
        self.assertIn("bash scripts/test-tailcat-bridge.sh", runs)
        self.assertNotIn("continue-on-error", job)
        for step in steps:
            self.assertNotIn("continue-on-error", step)
        self.assertNotIn("secrets.", str(job))
        self.assertNotIn("native-vm-browser", runs)

    def test_routed_lane_cannot_be_silently_skipped_at_required_gate(self):
        gate = self.ci["jobs"]["ci-gate"]
        self.assertIn("tailcat-tests", gate["needs"])
        step = next(s for s in gate["steps"] if s.get("name") == "verify required jobs passed or were intentionally skipped")
        self.assertEqual(step["env"]["TAILCAT_ROUTED"], "${{ needs.changes.outputs.tailcat }}")
        for routed, result, expected in [
            ("true", "success", 0), ("true", "skipped", 1),
            ("true", "failure", 1), ("true", "cancelled", 1),
            ("false", "skipped", 0), ("false", "failure", 1),
        ]:
            with self.subTest(routed=routed, result=result):
                script = re.sub(
                    r"\$\{\{ needs\.([\w-]+)\.result \}\}",
                    lambda m: result if m[1] == "tailcat-tests" else "success",
                    step["run"],
                )
                outcome = subprocess.run(
                    ["bash", "-c", script], capture_output=True, text=True,
                    env={**os.environ, "TAILCAT_ROUTED": routed, "WEB_ROUTED": "false",
                         "RUST_TEST_ROUTED": "false", "EVENT_NAME": "pull_request",
                         "HEAD_REPOSITORY": "fixture/labby", "BASE_REPOSITORY": "fixture/labby"},
                )
                self.assertEqual(outcome.returncode, expected, outcome.stdout + outcome.stderr)
        policy = self.ci["jobs"]["fleet-policy"]["steps"]
        self.assertTrue(any("scripts/ci/test_tailcat_workflow.py" in s.get("run", "") for s in policy))


if __name__ == "__main__":
    unittest.main()
