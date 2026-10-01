#!/usr/bin/env python3
"""Harness contract tests; fixtures are never release acceptance evidence."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("qualification", Path(__file__).with_name("qualify-first-use.py"))
qualification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualification)


class HarnessTests(unittest.TestCase):
    def state(self):
        return {"ready": True, "checks": [{"check": name, "status": "verified", "verified_at": 100,
                                         "resource_id": "public" if name == "catalog_search" else "actual"}
                                        for name in qualification.CHECKS]}

    def test_requires_all_fresh_real_checks(self):
        state = self.state()
        result = qualification.validate_readiness(state, 100, ["codex"], 101)
        self.assertEqual(set(result), qualification.CHECKS)
        self.assertNotIn("resource_id", result["agent_run"])
        for change in ({"status": "pending"}, {"status": "needs_recheck"}, {"verified_at": 99},
                       {"verified_at": 900}, {"resource_id": ""}, {"verified_at": True}):
            invalid = copy.deepcopy(state)
            invalid["checks"][0].update(change)
            with self.assertRaises(qualification.QualificationError):
                qualification.validate_readiness(invalid, 100, ["codex"], 101)
        for invalid in ({"ready": False, "checks": state["checks"]},
                        {"ready": True, "checks": state["checks"][:-1]},
                        {"ready": True, "checks": [state["checks"][0]] * 6}):
            with self.assertRaises(qualification.QualificationError):
                qualification.validate_readiness(invalid, 100, [], 101)

    def test_optional_clients_are_explicit_and_never_connected(self):
        state = self.state()
        client = next(check for check in state["checks"] if check["check"] == "selected_clients")
        client.update(status="deferred", verified_at=None, resource_id=None)
        self.assertEqual(qualification.validate_readiness(state, 100, [], 101)["selected_clients"]["status"], "deferred")
        with self.assertRaises(qualification.QualificationError):
            qualification.validate_readiness(state, 100, ["codex"], 101)

    def test_public_catalog_cannot_be_replaced_by_a_fixture_provider(self):
        state = self.state()
        next(check for check in state["checks"] if check["check"] == "catalog_search")["resource_id"] = "private-fixture"
        with self.assertRaises(qualification.QualificationError):
            qualification.validate_readiness(state, 100, [], 101)

    def test_private_token_rejects_shared_and_symlink_files(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "token"
            path.write_text("test-secret")
            path.chmod(0o600)
            self.assertEqual(qualification.read_private_token(path), "test-secret")
            link = Path(directory) / "link"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                qualification.read_private_token(link)
            path.chmod(0o644)
            with self.assertRaises(qualification.QualificationError):
                qualification.read_private_token(path)

    def test_only_activated_requested_release_receipt_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "labby"
            binary.write_bytes(b"candidate")
            metadata = Path(directory) / ".labby-install"
            metadata.mkdir()
            receipt = metadata / "receipt"
            digest = qualification.hashlib.sha256(b"candidate").hexdigest()
            receipt.write_text(f"format=1\nsource=release\nresolved_version=v1.2.3\nsha256={digest}\n")
            receipt.chmod(0o600)
            self.assertEqual(qualification.installed_release(binary, "v1.2.3"), digest)
            with self.assertRaises(qualification.QualificationError):
                qualification.installed_release(binary, "v1.2.4")
            binary.write_bytes(b"changed")
            with self.assertRaises(qualification.QualificationError):
                qualification.installed_release(binary, "v1.2.3")

    def test_mutating_run_requires_disposable_marker_and_reports_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            plan = {"conditions": dict.fromkeys(("fresh_machine", "cold_caches", "provider_access_ready", "native_service", "public_catalog", "provider_cost_accepted"), True),
                    "selected_clients": [], "stages": {stage: ["true"] for stage in qualification.STAGES},
                    "state_root": "/unused/server", "invoking_state_root": "/unused/user",
                    "install_dir": "/unused/bin", "token_file": "/unused/token", "gateway_url": "http://127.0.0.1:8765"}
            path, report = base / "plan.json", base / "report.json"
            path.write_text(json.dumps(plan))
            process = subprocess.run([sys.executable, str(Path(qualification.__file__)), "--plan", str(path),
                                      "--tag", "v1.2.3", "--report", str(report)], capture_output=True)
            self.assertEqual(process.returncode, 1)
            result = json.loads(report.read_text())
            self.assertFalse(result["completion"])
            self.assertIn("--disposable-machine", result["preflight_failure"])
            self.assertEqual(report.stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
