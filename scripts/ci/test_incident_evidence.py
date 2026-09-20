"""Negative evidence-contract tests; real daemon acceptance runs separately."""

import copy
import subprocess
import sys
import unittest
from pathlib import Path

if __package__:
    from .validate_incident_evidence import qualify_result
else:
    from validate_incident_evidence import qualify_result


class IncidentEvidenceTests(unittest.TestCase):
    def test_validator_remains_directly_executable(self):
        validator = Path(__file__).with_name("validate_incident_evidence.py")
        result = subprocess.run(
            [sys.executable, str(validator), "--help"],
            capture_output=True, text=True, timeout=10, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--evidence-dir", result.stdout)

    def test_ci_runs_negative_evidence_tests(self):
        workflow = (Path(__file__).resolve().parents[2] /
                    ".github/workflows/ci.yml").read_text()
        self.assertIn("scripts/ci/test_conformance_workflow.py", workflow)
        self.assertIn("scripts/ci/test_incident_evidence.py", workflow)

    def test_required_conformance_runs_actual_capture_pipeline(self):
        workflow = (Path(__file__).resolve().parents[2] /
                    ".github/workflows/verification-conformance.yml").read_text()
        self.assertIn("cargo build --manifest-path tools/verification/Cargo.toml -p labby-verify --locked", workflow)
        self.assertIn("timeout --signal=TERM --kill-after=5s 200s", workflow)
        self.assertIn("python3 scripts/ci/validate_incident_evidence.py", workflow)
        self.assertIn("--verifier tools/verification/target/debug/labby-verify", workflow)
        self.assertNotIn("continue-on-error:", workflow)

    def setUp(self):
        self.source = {"events": [{"fields": {"generation_id": "opaque-generation"}}]}
        self.result = {
            "incident": {
                "scenario": {
                    "project": "labby", "model": "browser_request", "invariant": "LABBY-REQ-005",
                    "fingerprint": "b3:" + "a" * 64,
                    "steps": [{"action": action} for action in
                              ["connect", "admit", "dispatch", "complete_success"]],
                    "origin": {"kind": "incident"}, "status": "unreproduced",
                    "expect": "invariant_violated",
                },
                "replay": {
                    "fingerprint": "b3:" + "a" * 64,
                    "verdict": "invariant_holds", "gate_failure": False,
                    "matches_expectation": False,
                    "observations": [{"position": i, "outcome": None if i == 0 else
                                      {"outcome": "applied"}} for i in range(5)],
                },
            },
            "exploration": {"backend": "stateright", "invariant": "LABBY-REQ-005",
                            "verdict": {"verdict": "bounded", "bounds": {"max_depth": 16}},
                            "scenarios": []},
        }

    def test_accepts_complete_applied_observed_prefix(self):
        qualify_result("success", self.source, self.result)

    def test_rejects_unreplayed_and_rejected_prefixes(self):
        for mutation in [
            lambda value: value["incident"]["replay"]["observations"].pop(),
            lambda value: value["incident"]["replay"]["observations"][2].update(
                outcome={"outcome": "rejected", "reason": "incorrect generation"}),
            lambda value: value["incident"]["scenario"]["steps"][1].update(action="cancel"),
        ]:
            result = copy.deepcopy(self.result)
            mutation(result)
            with self.assertRaises(ValueError):
                qualify_result("success", self.source, result)

    def test_rejects_proof_reproduction_and_raw_identifier_claims(self):
        for mutation in [
            lambda value: value["incident"]["scenario"].update(status="active"),
            lambda value: value["incident"]["replay"].update(matches_expectation=True),
            lambda value: value["exploration"].update(verdict={"verdict": "verified"}),
            lambda value: value["incident"]["scenario"]["steps"][0].update(
                generation="opaque-generation"),
        ]:
            result = copy.deepcopy(self.result)
            mutation(result)
            with self.assertRaises(ValueError):
                qualify_result("success", self.source, result)


if __name__ == "__main__":
    unittest.main()
