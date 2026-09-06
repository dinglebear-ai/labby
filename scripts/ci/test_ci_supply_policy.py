import datetime as dt
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("js_advisory_gate", ROOT / "scripts/ci/js_advisory_gate.py")
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class SupplyPolicyTests(unittest.TestCase):
    def test_inventory_covers_every_committed_manifest_and_lockfile_once(self):
        policy = GATE.load_policy(ROOT / "scripts/ci/js-advisory-policy.json")
        tracked = GATE.tracked_dependency_files(ROOT)
        with mock.patch.object(GATE, "tracked_dependency_files", return_value=tracked) as inventory:
            self.assertEqual(GATE.validate_inventory(ROOT, policy), [])
        inventory.assert_called_once_with(ROOT)

    def test_high_advisory_blocks_and_active_exception_is_auditable(self):
        payload = {"vulnerabilities": {"bad-package": {"severity": "high", "via": [{"source": 123, "severity": "high", "url": "https://example.invalid/123", "range": "<2"}]}}}
        policy = {"minimum_severity": "high", "ignored_advisories": []}
        self.assertEqual(GATE.blocking_advisories(payload, policy)[0]["id"], "123")
        policy["ignored_advisories"] = [{"id": "123", "rationale": "mitigated", "expires": "2999-01-01"}]
        self.assertEqual(GATE.blocking_advisories(payload, policy), [])

    def test_expired_or_unexplained_exception_is_rejected(self):
        base = {"schema_version": 1, "minimum_severity": "high", "workspaces": []}
        for ignored in [
            {"id": "123", "rationale": "", "expires": "2999-01-01"},
            {"id": "123", "rationale": "temporary", "expires": (dt.date.today() - dt.timedelta(days=1)).isoformat()},
        ]:
            with self.subTest(ignored=ignored), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "policy.json"
                path.write_text(json.dumps({**base, "ignored_advisories": [ignored]}))
                with self.assertRaises(ValueError):
                    GATE.load_policy(path)


if __name__ == "__main__":
    unittest.main()
