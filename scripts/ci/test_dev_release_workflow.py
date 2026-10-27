"""The dev release PR shares publication ownership with manual preparation."""
from pathlib import Path
import sys
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]


class DevReleaseWorkflow(unittest.TestCase):
    def test_generated_request_in_this_checkout_is_valid_before_merge(self):
        if not (ROOT / '.release-please-dev.json').exists():
            self.skipTest('this checkout is not a generated dev request')
        sys.path.insert(0, str(ROOT / 'scripts/ci'))
        from dev_release_pr import Coordinator
        Coordinator(ROOT, 'example/labby').request('HEAD')

    def test_coordinator_waits_for_stable_metadata_without_changing_stable_jobs(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/release-please.yml').read_text())
        job = workflow['jobs']['dev-release-pr']
        self.assertEqual(['stabilize-release-pr', 'release-please', 'sync-release-version'], job['needs'])
        self.assertIn("needs.sync-release-version.result == 'success'", job['if'])
        self.assertIn("needs.release-please.result == 'success'", job['if'])
        self.assertIn('!cancelled()', job['if'])
        self.assertIn("github.ref == 'refs/heads/main'", job['if'])
        self.assertEqual('ubuntu-24.04', job['runs-on'])
        self.assertLessEqual(job['timeout-minutes'], 20)

    def test_dev_coordinator_and_manual_tag_preparation_share_serialization(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/release-please.yml').read_text())
        manual = yaml.safe_load((ROOT / '.github/workflows/dev-release.yml').read_text())
        self.assertEqual(manual['concurrency'], workflow['jobs']['dev-release-pr']['concurrency'])
        self.assertIs(False, manual['concurrency']['cancel-in-progress'])

    def test_exact_green_source_and_supported_credentials_reach_coordinator(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/release-please.yml').read_text())
        job = workflow['jobs']['dev-release-pr']
        checkout, coordinate = job['steps']
        self.assertEqual('main', checkout['with']['ref'])
        self.assertEqual(0, checkout['with']['fetch-depth'])
        self.assertIn('github.event.workflow_run.head_sha', coordinate['env']['SOURCE_SHA'])
        self.assertEqual('${{ secrets.RELEASE_PLEASE_TOKEN }}', coordinate['env']['GH_TOKEN'])
        self.assertIn('--source "$SOURCE_SHA"', coordinate['run'])
        self.assertIn('scripts/ci/dev_release_pr.py', coordinate['run'])
        self.assertNotIn('gh release', coordinate['run'])
        self.assertNotIn('gh pr merge', coordinate['run'])


if __name__ == '__main__':
    unittest.main()
