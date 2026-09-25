from pathlib import Path
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
WORKFLOW_DIR = ROOT / ".github" / "workflows"


def job_block(workflow: str, job: str, next_job: str) -> str:
    start = workflow.index(f"  {job}:\n")
    end = workflow.index(f"  {next_job}:\n", start)
    return workflow[start:end]


class WindowsCiPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_workspace_windows_job_is_hosted_cached_and_bounded(self) -> None:
        block = job_block(self.workflow, "test-windows", "release-contract")
        self.assertIn("runs-on: windows-latest", block)
        self.assertNotIn("self-hosted", block)
        self.assertIn("timeout-minutes: 60", block)
        self.assertIn("Swatinem/rust-cache@", block)
        self.assertIn("key: workspace-native-windows-v2", block)
        self.assertIn("cache-on-failure: true", block)
        self.assertIn("cargo test --workspace --all-features --locked --no-run", block)
        self.assertIn("shard: [1, 2, 3, 4]", block)
        self.assertIn("--partition hash:${{ matrix.shard }}/4", block)
        self.assertIn("if: matrix.shard == 1", block)
        self.assertIn("--test windows_job_object_reaping", block)
        self.assertIn("--run-ignored ignored-only", block)
        self.assertNotIn("--no-tests pass", block)

    def test_desktop_windows_job_is_hosted_cached_and_bounded(self) -> None:
        block = job_block(self.workflow, "desktop-windows", "verification-t0")
        self.assertIn("runs-on: windows-latest", block)
        self.assertIn("timeout-minutes: 60", block)
        self.assertIn("Swatinem/rust-cache@", block)
        self.assertIn("key: labby-desktop-windows-v1", block)
        self.assertIn("cache-on-failure: true", block)

    def test_native_containment_is_not_replaced_by_unix_supervisor_checks(self) -> None:
        block = job_block(self.workflow, "test-windows", "release-contract")
        self.assertIn(
            "run: cargo nextest run --workspace --all-features --locked --profile ci "
            "--test-threads 4 --partition hash:${{ matrix.shard }}/4\n",
            block,
        )
        self.assertIn(
            "run: cargo nextest run -p labby --test windows_job_object_reaping "
            "--all-features --locked --profile ci --run-ignored ignored-only\n",
            block,
        )
        self.assertNotIn("continue-on-error: true", block)

    def test_windows_jobs_run_only_when_explicitly_dispatched(self) -> None:
        self.assertIn("      run_windows:\n", self.workflow)
        self.assertIn("        type: boolean\n", self.workflow)
        self.assertIn("        default: false\n", self.workflow)

        installer = job_block(self.workflow, "windows-installer", "macos-installer")
        windows = job_block(self.workflow, "test-windows", "release-contract")
        desktop = job_block(self.workflow, "desktop-windows", "verification-t0")
        manual_gate = "github.event_name == 'workflow_dispatch' && inputs.run_windows == true"
        self.assertIn(manual_gate, installer)
        self.assertIn(manual_gate, windows)
        self.assertIn(manual_gate, desktop)

        for block in (installer, windows, desktop):
            self.assertNotIn("github.event_name != 'pull_request'", block)

    def test_workspace_windows_job_remains_visible_to_ci_gate_when_skipped(self) -> None:
        block = self.workflow[self.workflow.index("  ci-gate:\n") :]
        self.assertIn("      - test-windows\n", block)
        self.assertNotIn("      - desktop-windows\n", block)
        self.assertIn("needs.test-windows.result", block)
        self.assertNotIn("needs.desktop-windows.result", block)

    def test_self_hosted_jobs_are_declared_non_blocking_and_same_repository(self) -> None:
        # Hosted runners stay the default. The self-hosted fleet is privileged
        # (DinD on an Unraid host), so a job may only target it when it cannot
        # gate a merge and cannot execute fork-PR code.
        allowed = {("ci.yml", "test")}
        for path in WORKFLOW_DIR.glob("*.y*ml"):
            workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
            for name, job in (workflow.get("jobs") or {}).items():
                runner = job.get("runs-on")
                labels = runner if isinstance(runner, list) else [runner]
                if not any(str(label).startswith("ci-pool-") or label == "self-hosted" for label in labels):
                    continue
                self.assertIn((path.name, name), allowed, f"{path.name}:{name} may not use the self-hosted fleet")
                self.assertIs(True, job.get("continue-on-error"), f"{path.name}:{name} must be non-blocking")
                self.assertIn(
                    "github.event.pull_request.head.repo.full_name == github.repository",
                    str(job.get("if", "")),
                    f"{path.name}:{name} must refuse fork pull requests",
                )

    def test_ci_gate_waits_for_required_suites_but_not_advisory_jobs(self) -> None:
        gate = yaml.safe_load(self.workflow)["jobs"]["ci-gate"]["needs"]
        for job in ("rust-coverage", "live-e2e-core"):
            self.assertNotIn(job, yaml.safe_load(self.workflow)["jobs"], f"{job} must run independently of CI")
            self.assertNotIn(job, gate, f"{job} remains advisory")
        for job in ("test", "feature-slices", "test-windows", "test-fork", "clippy"):
            self.assertIn(job, gate)


if __name__ == "__main__":
    unittest.main()
