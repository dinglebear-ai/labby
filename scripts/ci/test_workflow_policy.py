import unittest
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

    def test_policy_rejects_unreviewed_download_artifact_revisions(self):
        errors = policy.external_use_errors(
            Path("fixture.yml"), "actions/download-artifact@" + "0" * 40
        )
        self.assertEqual(len(errors), 1)
        self.assertIn("must use reviewed revision", errors[0])


if __name__ == "__main__":
    unittest.main()
