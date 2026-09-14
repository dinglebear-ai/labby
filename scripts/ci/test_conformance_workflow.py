"""Enforce the required real-process lifecycle conformance CI contract."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from typing import Any, ClassVar

import yaml


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR = ROOT / "scripts/ci/validate_conformance_evidence.py"


class ConformanceWorkflowTests(unittest.TestCase):
    workflow: ClassVar[dict[str, Any]]

    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = yaml.load(
            (ROOT / ".github/workflows/verification-conformance.yml").read_text(),
            Loader=yaml.BaseLoader,
        )

    def job(self) -> dict[str, Any]:
        return self.workflow["jobs"]["conformance"]

    def test_is_reusable_credentialless_and_required(self) -> None:
        self.assertIn("workflow_call", self.workflow["on"])
        self.assertEqual(self.workflow["permissions"], {"contents": "read"})
        job = self.job()
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertIn("Required", job["name"])
        self.assertEqual(job["env"]["LABBY_CONFORMANCE_REQUIRED"], "1")
        self.assertEqual(
            job["env"]["LABBY_CONFORMANCE_EVIDENCE_DIR"],
            "${{ github.workspace }}/verification-conformance-evidence",
        )
        self.assertEqual(job["steps"][0]["with"]["persist-credentials"], "false")
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)

    def test_caller_requires_conformance_without_path_or_skip_waiver(self) -> None:
        caller = yaml.load((ROOT / ".github/workflows/ci.yml").read_text(), Loader=yaml.BaseLoader)
        job = caller["jobs"]["verification-conformance"]
        self.assertEqual(job["uses"], "./.github/workflows/verification-conformance.yml")
        self.assertNotIn("if", job)
        self.assertNotIn("continue-on-error", job)
        gate = caller["jobs"]["ci-gate"]
        self.assertIn("verification-conformance", gate["needs"])
        script = "\n".join(step.get("run", "") for step in gate["steps"])
        self.assertIn('require_success verification-conformance "${{ needs.verification-conformance.result }}"', script)

    def test_build_is_separate_from_five_minute_execution(self) -> None:
        steps = self.job()["steps"]
        build = next(step for step in steps if step.get("name", "").startswith("Build product"))
        self.assertEqual(build["timeout-minutes"], "15")
        self.assertIn("cargo build -p labby --all-features --bin labby --locked", build["run"])
        self.assertIn(
            "cargo test -p labby --all-features --test lifecycle_conformance --no-run --locked",
            build["run"],
        )
        execute = next(step for step in steps if step.get("name", "").startswith("Run required"))
        self.assertEqual(execute["timeout-minutes"], "6")
        self.assertIn("timeout --signal=TERM --kill-after=5s 300s", execute["run"])
        self.assertIn(
            "cargo test -p labby --all-features --test lifecycle_conformance --locked --",
            execute["run"],
        )
        self.assertIn("--test-threads=1 conformance", execute["run"])
        self.assertNotIn("--no-run", execute["run"])

    def test_provenance_capture_does_not_make_clean_checkout_dirty(self) -> None:
        source = next(
            step for step in self.job()["steps"]
            if step.get("name", "").startswith("Capture source")
        )
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            subprocess.run(["git", "init", "--quiet"], cwd=directory, check=True)
            subprocess.run(
                [
                    "git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                    "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                    "commit", "--allow-empty", "--quiet", "-m", "fixture",
                ],
                cwd=directory,
                check=True,
            )
            evidence_dir = directory / "verification-conformance-evidence"
            subprocess.run(
                ["bash", "-c", source["run"]],
                cwd=directory,
                env={"PATH": os.environ["PATH"],
                     "LABBY_CONFORMANCE_EVIDENCE_DIR": str(evidence_dir)},
                check=True,
            )
            self.assertEqual(
                subprocess.run(
                    ["git", "status", "--porcelain"],
                    cwd=directory,
                    text=True,
                    capture_output=True,
                    check=True,
                ).stdout,
                "",
            )
            self.assertEqual((evidence_dir / "source-dirty.txt").read_text(), "")

    def test_retains_source_binary_trace_and_cleanup_evidence(self) -> None:
        steps = self.job()["steps"]
        source = next(step for step in steps if step.get("name", "").startswith("Capture source"))
        self.assertIn("git status --porcelain", source["run"])
        self.assertIn("/verification-conformance-evidence/", source["run"])
        self.assertIn("source-revision.txt", source["run"])
        self.assertIn("source-dirty.txt", source["run"])
        binary = next(step for step in steps if step.get("name", "").startswith("Capture real"))
        self.assertIn("sha256sum target/debug/labby", binary["run"])
        evidence = next(step for step in steps if step.get("name", "").startswith("Require trace"))
        self.assertEqual(evidence["if"], "success()")
        self.assertIn("scripts/ci/validate_conformance_evidence.py", evidence["run"])
        self.assertIn('--evidence-dir "$LABBY_CONFORMANCE_EVIDENCE_DIR"', evidence["run"])
        upload = next(step for step in steps if "upload-artifact" in step.get("uses", ""))
        self.assertEqual(upload["if"], "always()")
        self.assertEqual(upload["with"]["name"], "verification-conformance")
        self.assertEqual(upload["with"]["path"], "verification-conformance-evidence/")
        self.assertEqual(upload["with"]["if-no-files-found"], "error")
        self.assertEqual(upload["with"]["retention-days"], "14")

    def evidence_fixture(self, directory: Path) -> dict[str, dict[str, Any]]:
        git_sha = "a" * 40
        binary_sha = "b" * 64
        (directory / "source-revision.txt").write_text(f"{git_sha}\n")
        (directory / "source-dirty.txt").write_text("")
        (directory / "binary-sha256.txt").write_text(f"{binary_sha}  target/debug/labby\n")

        actions = {
            "success": ["admit", "dispatch", "complete_success"],
            "disconnect": ["admit", "dispatch", "disconnect"],
            "tool-error": ["admit", "dispatch", "complete_error"],
            "timeout": ["admit", "dispatch", "timeout"],
            "document-invalidation": ["admit", "dispatch", "invalidate_document"],
            "generation-replacement": ["admit", "dispatch", "replace_connection", "admit", "dispatch", "complete_success"],
            "admit-cancel-late-cleanup": ["admit", "cancel", "complete_success"],
            "dispatch-cancel-late-cleanup": ["admit", "dispatch", "cancel", "complete_success"],
            "divergent-adapter-self-test": ["admit", "dispatch", "complete_success"],
        }
        records = {}
        for case_id, sequence in actions.items():
            negative = case_id == "divergent-adapter-self-test"
            steps = [{"action": action} for action in sequence]
            encoded = json.dumps(steps, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
            records[f"{case_id}.json"] = {
                "schema_version": 1, "lane": "conformance", "case_id": case_id,
                "evidence_kind": "negative_adapter_self_test" if negative else "real_process",
                "relation": "browser-public-v1",
                "trace_fingerprint": hashlib.sha256(encoded).hexdigest(),
                "steps": steps,
                "observations": [
                    {"position": position, "relation_passed": not (negative and position == 3)}
                    for position in range(len(steps) + 1)
                ],
                "failure": {"kind": "observation_divergence", "position": 3} if negative else None,
                "source": {"git_sha": git_sha, "git_dirty": False, "binary_sha256": binary_sha,
                           "fixture_versions": ["fixture:v1"]},
                "cleanup": {"clean": True, "failures": []}, "verdict": "passed",
            }
        return records

    def run_validator(
        self, directory: Path, records: dict[str, dict[str, Any]]
    ) -> subprocess.CompletedProcess[str]:
        for name, record in records.items():
            (directory / name).write_text(json.dumps(record))
        return subprocess.run(
            ["python3", str(VALIDATOR), "--evidence-dir", str(directory)],
            text=True,
            capture_output=True,
            check=False,
        )

    def test_validator_accepts_complete_bound_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            result = self.run_validator(directory, self.evidence_fixture(directory))
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_validator_rejects_missing_case_or_unobserved_negative_divergence(self) -> None:
        mutations = [
            lambda records: records.pop("success.json"),
            lambda records: records["divergent-adapter-self-test.json"].update(failure=None),
            lambda records: records["divergent-adapter-self-test.json"]["observations"][3].update(relation_passed=True),
            lambda records: records["divergent-adapter-self-test.json"]["source"].update(binary_sha256="d" * 64),
        ]
        for mutate in mutations:
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                records = self.evidence_fixture(directory)
                mutate(records)
                self.assertNotEqual(self.run_validator(directory, records).returncode, 0)

    def test_validator_rejects_nonregular_file_and_wrong_action_sequence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            records = self.evidence_fixture(directory)
            records.pop("success.json")
            (directory / "success.json").mkdir()
            self.assertNotEqual(self.run_validator(directory, records).returncode, 0)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            records = self.evidence_fixture(directory)
            records["success.json"]["steps"][2]["action"] = "cancel"
            encoded = json.dumps(records["success.json"]["steps"], sort_keys=True, separators=(",", ":")).encode()
            records["success.json"]["trace_fingerprint"] = hashlib.sha256(encoded).hexdigest()
            self.assertNotEqual(self.run_validator(directory, records).returncode, 0)

    def test_validator_rejects_wrong_provenance_and_arbitrary_hash(self) -> None:
        mutations = {
            "wrong revision": lambda record: record["source"].update(git_sha="c" * 40),
            "wrong dirty state": lambda record: record["source"].update(git_dirty=True),
            "wrong binary": lambda record: record["source"].update(binary_sha256="d" * 64),
            "arbitrary trace hash": lambda record: record.update(trace_fingerprint="e" * 64),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                records = self.evidence_fixture(directory)
                mutate(records["dispatch-cancel-late-cleanup.json"])
                result = self.run_validator(directory, records)
                self.assertNotEqual(result.returncode, 0)

    def test_validator_rejects_sparse_duplicate_or_failed_observations(self) -> None:
        mutations = {
            "sparse": [{"position": 0, "relation_passed": True}],
            "duplicate": [
                {"position": 0, "relation_passed": True},
                {"position": 0, "relation_passed": True},
            ],
            "relation failed": [
                {"position": 0, "relation_passed": True},
                {"position": 1, "relation_passed": False},
            ],
        }
        for name, observations in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                records = self.evidence_fixture(directory)
                records["dispatch-cancel-late-cleanup.json"]["observations"] = observations
                result = self.run_validator(directory, records)
                self.assertNotEqual(result.returncode, 0)

    def test_validator_rejects_incomplete_required_case(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            records = self.evidence_fixture(directory)
            records["dispatch-cancel-late-cleanup.json"]["verdict"] = "incomplete"
            result = self.run_validator(directory, records)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("required case did not pass", result.stderr)

    def test_validator_rejects_boolean_position_and_malformed_cleanup(self) -> None:
        mutations = {
            "boolean position": lambda record: record["observations"][1].update(position=True),
            "cleanup list": lambda record: record.update(cleanup=[]),
            "unhashable case id": lambda record: record.update(case_id=[]),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                records = self.evidence_fixture(directory)
                mutate(records["dispatch-cancel-late-cleanup.json"])
                result = self.run_validator(directory, records)
                self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
