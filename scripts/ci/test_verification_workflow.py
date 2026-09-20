"""Ensure the advisory toolkit lane runs assertions without repairing fixtures."""

from pathlib import Path
import subprocess
import tempfile
import unittest
from typing import Any, ClassVar

import yaml

from scripts.ci.changed_paths import classify


ROOT = Path(__file__).resolve().parents[2]


class VerificationWorkflowTests(unittest.TestCase):
    workflow: ClassVar[dict[str, Any]]
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = yaml.load(
            (ROOT / ".github/workflows/verification.yml").read_text(),
            Loader=yaml.BaseLoader,
        )

    def test_triggers_include_inputs_and_canonical_verification_docs(self) -> None:
        paths = self.workflow["on"]["pull_request"]["paths"]
        for required in ["tools/verification/**", "docs/dev/VERIFICATION.md",
                         "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/**",
                         "clippy.toml", "Justfile", ".github/actions/setup-rust-kache/**",
                         ".github/workflows/verification.yml",
                         "scripts/ci/test_verification_workflow.py"]:
            self.assertIn(required, paths)
        self.assertEqual(self.workflow["on"]["push"]["branches"], ["main"])
        self.assertIn("workflow_dispatch", self.workflow["on"])

    def test_lane_is_bounded_credentialless_and_visibly_advisory(self) -> None:
        self.assertEqual(self.workflow["permissions"], {"contents": "read"})
        job = self.workflow["jobs"]["core"]
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertEqual(job["timeout-minutes"], "15")
        self.assertIn("advisory", job["name"])
        self.assertNotIn("continue-on-error", job)
        steps = job["steps"]
        self.assertEqual(steps[0]["with"]["persist-credentials"], "false")
        self.assertEqual(steps[1]["uses"], "./.github/actions/setup-rust-kache")

    def test_schema_is_asserted_not_regenerated_in_ci(self) -> None:
        steps = self.workflow["jobs"]["core"]["steps"]
        runs = [step["run"] for step in steps if "run" in step]
        self.assertIn("cargo test --manifest-path tools/verification/Cargo.toml --workspace --all-features --locked", runs)
        self.assertIn("python3 -m unittest discover -s tools/verification/tests -p 'test_*.py' -v", runs)
        for step in steps:
            self.assertNotIn("continue-on-error", step)
            self.assertNotIn("--write", step.get("run", ""))
            self.assertNotIn("verify-schema", step.get("run", ""))

    def test_product_gates_only_follow_consumed_pure_toolkit_leaves(self) -> None:
        for path in ["tools/verification/Cargo.lock",
                     "tools/verification/crates/verify-runner/src/registry.rs"]:
            result = classify("pull_request", [path])
            self.assertFalse(result["rust_compile"])
            self.assertFalse(result["rust_test"])
        for path in ["tools/verification/Cargo.toml", "tools/verification/crates/verify-core/src/catalog.rs",
                     "tools/verification/crates/verify-scenario/src/envelope.rs"]:
            result = classify("pull_request", [path])
            self.assertTrue(result["rust_compile"])
            self.assertTrue(result["rust_test"])
        self.assertTrue(classify("pull_request", ["scripts/ci/test_verification_workflow.py"])["workflow"])

    def test_isolated_lockfile_is_audited(self) -> None:
        steps = self.workflow["jobs"]["core"]["steps"]
        installer = next(step for step in steps
                         if step.get("with", {}).get("tool") == "cargo-deny@0.20.2")
        self.assertEqual(installer["uses"],
                         "taiki-e/install-action@3ae2e1de8b1f6447853fd29a6000b3953b0494ac")
        command = "cargo deny --manifest-path tools/verification/Cargo.toml --config tools/verification/deny.toml --locked check"
        audit = next(step for step in steps if step.get("run") == command)
        self.assertLess(steps.index(installer), steps.index(audit))

    def test_t0_is_unconditional_required_reusable_and_bounded(self) -> None:
        ci = yaml.load((ROOT / ".github/workflows/ci.yml").read_text(), Loader=yaml.BaseLoader)
        job = ci["jobs"]["verification-t0"]
        self.assertEqual(job["uses"], "./.github/workflows/verification-t0.yml")
        self.assertNotIn("if", job)
        self.assertNotIn("needs", job)
        gate = ci["jobs"]["ci-gate"]
        self.assertIn("verification-t0", gate["needs"])
        self.assertTrue(any('require_success verification-t0 "${{ needs.verification-t0.result }}"'
                            in step.get("run", "") for step in gate["steps"]))
        t0 = yaml.load((ROOT / ".github/workflows/verification-t0.yml").read_text(), Loader=yaml.BaseLoader)
        self.assertIn("workflow_call", t0["on"])
        self.assertEqual(t0["permissions"], {"contents": "read"})
        replay = t0["jobs"]["replay"]
        self.assertEqual(replay["timeout-minutes"], "20")
        component_minutes = sum(int(step.get("timeout-minutes", "0")) for step in replay["steps"])
        self.assertGreater(int(replay["timeout-minutes"]), component_minutes + 1)
        self.assertEqual(replay["steps"][0]["with"]["persist-credentials"], "false")
        runs = "\n".join(s.get("run", "") for s in replay["steps"])
        self.assertIn("--kill-after=5s 60s", runs)
        self.assertIn("labby-verify t0 tools/verification/formal", runs)
        self.assertIn("git rev-parse HEAD", runs)
        self.assertIn("sha256sum", runs)
        audit = "cargo deny --manifest-path tools/verification/Cargo.toml --config tools/verification/deny.toml --locked check"
        self.assertIn(audit, runs)
        installer = next(step for step in replay["steps"]
                         if step.get("with", {}).get("tool") == "cargo-deny@0.20.2")
        self.assertEqual(installer["uses"], "taiki-e/install-action@3ae2e1de8b1f6447853fd29a6000b3953b0494ac")
        for step in replay["steps"]:
            self.assertNotIn("continue-on-error", step)

    def test_t0_provenance_does_not_make_clean_source_dirty(self) -> None:
        t0 = yaml.load((ROOT / ".github/workflows/verification-t0.yml").read_text(), Loader=yaml.BaseLoader)
        command = next(s["run"] for s in t0["jobs"]["replay"]["steps"]
                       if "git status" in s.get("run", ""))
        capture = command.split("sha256sum", 1)[0]
        for dirty in (False, True):
            with self.subTest(dirty=dirty), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                subprocess.run(["git", "init", "--quiet", directory], check=True, capture_output=True)
                subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                                "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                                "commit", "--allow-empty", "--quiet", "-m", "fixture"],
                               cwd=root, check=True, capture_output=True)
                if dirty:
                    (root / "legitimate-change.txt").write_text("untracked source change")
                subprocess.run(["bash", "-c", capture], cwd=root, check=True, capture_output=True)
                status = (root / "verification-evidence/source-dirty.txt").read_text().strip()
                self.assertEqual(status, "?? legitimate-change.txt" if dirty else "")

    def test_t1_is_reusable_advisory_with_separate_execution_budget(self) -> None:
        t1 = yaml.load((ROOT / ".github/workflows/verification-t1.yml").read_text(), Loader=yaml.BaseLoader)
        self.assertIn("workflow_call", t1["on"])
        self.assertEqual(t1["permissions"], {"contents": "read"})
        job = t1["jobs"]["check"]
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertIn("advisory", job["name"])
        self.assertGreater(int(job["timeout-minutes"]), 2 + 12 + 5)
        runs = "\n".join(step.get("run", "") for step in job["steps"])
        self.assertIn("--kill-after=5s 290s", runs)
        self.assertIn("labby-verify t1 tools/verification/formal", runs)
        self.assertIn("-p verify-loom", runs)
        self.assertIn("--test lifecycle loom_", runs)
        self.assertIn("cargo deny --manifest-path tools/verification/Cargo.toml --config tools/verification/deny.toml --locked check", runs)
        self.assertEqual(job["steps"][0]["with"]["persist-credentials"], "false")
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)
        ci = yaml.load((ROOT / ".github/workflows/ci.yml").read_text(), Loader=yaml.BaseLoader)
        call = ci["jobs"]["verification-t1"]
        self.assertEqual(call["uses"], "./.github/workflows/verification-t1.yml")
        self.assertNotIn("if", call)
        self.assertNotIn("verification-t1", ci["jobs"]["ci-gate"]["needs"])

    def test_t2_runs_exact_kani_controls_on_a_finite_advisory_schedule(self) -> None:
        t2 = yaml.load((ROOT / ".github/workflows/verification-t2.yml").read_text(),
                       Loader=yaml.BaseLoader)
        self.assertIn("workflow_call", t2["on"])
        self.assertIn("workflow_dispatch", t2["on"])
        self.assertEqual(len(t2["on"]["schedule"]), 1)
        self.assertEqual(t2["permissions"], {"contents": "read"})
        job = t2["jobs"]["kani"]
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertEqual(job["timeout-minutes"], "45")
        self.assertIn("advisory", job["name"])
        self.assertEqual(job["steps"][0]["with"]["persist-credentials"], "false")
        runs = "\n".join(step.get("run", "") for step in job["steps"])
        self.assertIn("--version 0.67.0 kani-verifier", runs)
        self.assertIn("cargo kani setup", runs)
        self.assertIn('$(dirname "$driver")', runs)
        self.assertIn('>> "$GITHUB_PATH"', runs)
        self.assertIn("--kill-after=5s 1200s", runs)
        self.assertIn("--test actual_kani", runs)
        self.assertIn("--ignored --test-threads=1", runs)
        self.assertIn("sha256sum", runs)
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)
        shuttle = t2["jobs"]["shuttle"]
        self.assertEqual(shuttle["timeout-minutes"], "45")
        self.assertEqual(shuttle["steps"][0]["with"]["persist-credentials"], "false")
        shuttle_runs = "\n".join(step.get("run", "") for step in shuttle["steps"])
        self.assertIn("--kill-after=5s 2100s", shuttle_runs)
        self.assertIn("-p verify-loom", shuttle_runs)
        self.assertIn("--test lifecycle shuttle_", shuttle_runs)
        for step in shuttle["steps"]:
            self.assertNotIn("continue-on-error", step)

    def test_t3_authenticates_and_runs_exact_formal_tool_controls(self) -> None:
        t3 = yaml.load((ROOT / ".github/workflows/verification-t3.yml").read_text(),
                       Loader=yaml.BaseLoader)
        self.assertIn("workflow_call", t3["on"])
        self.assertIn("workflow_dispatch", t3["on"])
        self.assertEqual(len(t3["on"]["schedule"]), 1)
        self.assertEqual(t3["permissions"], {"contents": "read"})
        job = t3["jobs"]["formal-tools"]
        self.assertEqual(job["runs-on"], "ubuntu-24.04")
        self.assertEqual(job["timeout-minutes"], "60")
        self.assertIn("advisory", job["name"])
        self.assertEqual(job["steps"][0]["with"]["persist-credentials"], "false")
        runs = "\n".join(step.get("run", "") for step in job["steps"])
        self.assertIn("releases/download/v1.7.4/tla2tools.jar", runs)
        self.assertIn("releases/download/v6.2.0/org.alloytools.alloy.dist.jar", runs)
        self.assertIn("sha256sum --check", runs)
        self.assertIn('test "$tlc_status" -eq 1', runs)
        self.assertIn("grep -F 'Version 2.19'", runs)
        self.assertIn("--kill-after=5s 2700s", runs)
        self.assertIn("-p verify-tla -p verify-alloy", runs)
        self.assertIn("--test actual_tools", runs)
        self.assertIn("--ignored --test-threads=1", runs)
        self.assertIn("Apalache unavailable", runs)
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)

    def test_reporting_preserves_original_results_and_commenting_is_opt_in(self) -> None:
        for tier, job_name in [("t0", "replay"), ("t1", "check")]:
            workflow = yaml.load((ROOT / f".github/workflows/verification-{tier}.yml").read_text(), Loader=yaml.BaseLoader)
            steps = workflow["jobs"][job_name]["steps"]
            render = next(step for step in steps if f"report-{tier}" in step.get("run", ""))
            self.assertEqual(render["if"], "always()")
            self.assertIn("json text markdown html", render["run"])
            self.assertIn("GITHUB_STEP_SUMMARY", render["run"])
            self.assertNotIn("continue-on-error", render)
        publisher = yaml.load((ROOT / ".github/workflows/verification-report.yml").read_text(), Loader=yaml.BaseLoader)
        self.assertEqual(publisher["on"]["workflow_call"]["inputs"]["publish-comment"]["default"], "false")
        job = publisher["jobs"]["comment"]
        self.assertIn("inputs.publish-comment", job["if"])
        self.assertIn("head.repo.full_name == github.repository", job["if"])
        self.assertEqual(job["timeout-minutes"], "2")
        self.assertFalse(any("checkout" in step.get("uses", "") for step in job["steps"]))
        script = job["steps"][-1]["with"]["script"]
        self.assertIn("stat.size > 60000", script)
        self.assertIn("issue_number: context.issue.number", script)


if __name__ == "__main__":
    unittest.main()
