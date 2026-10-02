import re
import unittest

import yaml
from pathlib import Path

from scripts.ci import check_workflow_policy as policy


class WorkflowPolicyTests(unittest.TestCase):
    def test_repository_uses_one_reviewed_download_artifact_revision(self):
        root = Path(__file__).resolve().parents[2] / ".github/workflows"
        uses = []
        for path in root.glob("*.yml"):
            uses.extend(
                line.split("actions/download-artifact@", 1)[1].split()[0]
                for line in path.read_text().splitlines()
                if "actions/download-artifact@" in line
            )
        self.assertTrue(uses, "workflow scan must not pass vacuously")
        self.assertEqual(set(uses), {policy.DOWNLOAD_ARTIFACT_SHA})

    def test_live_e2e_build_and_qualification_budgets_are_separate(self):
        root = Path(__file__).resolve().parents[2]
        workflow = yaml.load((root / ".github/workflows/live-e2e.yml").read_text(), Loader=yaml.BaseLoader)
        job = workflow["jobs"]["live-e2e-core"]
        build = next(step for step in job["steps"] if step.get("name") == "Precompile product and live E2E test targets")
        run = next(step for step in job["steps"] if step.get("name") == "Run bounded hermetic product E2E shards")
        self.assertLess(job["steps"].index(build), job["steps"].index(run))
        self.assertEqual(build["timeout-minutes"], "20")
        self.assertIn("cargo build -p labby --all-features --bin labby --locked", build["run"])
        self.assertIn("--no-run", build["run"])
        harness_targets = set(re.findall(r"--test ([a-z_]+)", (root / "scripts/ci/labby-live-e2e.sh").read_text()))
        self.assertTrue(harness_targets)
        self.assertEqual(set(re.findall(r"--test ([a-z_]+)", build["run"])), harness_targets)
        self.assertEqual(run["env"]["LABBY_E2E_PREBUILT"], "1")
        self.assertEqual(run["env"]["LABBY_E2E_SHARD_TIMEOUT_SECONDS"], "900")
        self.assertEqual(run["env"]["LABBY_E2E_RUN_TIMEOUT_SECONDS"], "1800")
        self.assertEqual(run["timeout-minutes"], "31")
        self.assertEqual(job["timeout-minutes"], "60")
        self.assertEqual(build["env"]["CARGO_BUILD_JOBS"], run["env"]["CARGO_BUILD_JOBS"])
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertEqual(job["steps"][0]["with"]["persist-credentials"], "false")
        for step in (build, run):
            self.assertNotIn("continue-on-error", step)
        upload = job["steps"][-1]
        self.assertEqual(upload["if"], "${{ always() }}")
        self.assertEqual(upload["with"]["if-no-files-found"], "error")

    def test_policy_rejects_unreviewed_download_artifact_revisions(self):
        errors = policy.external_use_errors(
            Path("fixture.yml"), "actions/download-artifact@" + "0" * 40
        )
        self.assertEqual(len(errors), 1)
        self.assertIn("must use reviewed revision", errors[0])


if __name__ == "__main__":
    unittest.main()
