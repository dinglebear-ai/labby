from pathlib import Path
import unittest


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
        self.assertIn("key: workspace-nextest-v1", block)
        self.assertIn("cache-on-failure: true", block)

    def test_palette_windows_job_is_hosted_cached_and_bounded(self) -> None:
        block = job_block(self.workflow, "palette-windows", "rust-coverage")
        self.assertIn("runs-on: windows-latest", block)
        self.assertIn("timeout-minutes: 60", block)
        self.assertIn("Swatinem/rust-cache@", block)
        self.assertIn("key: palette-tauri-windows-v1", block)
        self.assertIn("cache-on-failure: true", block)

    def test_workspace_windows_remains_advisory_to_ci_gate(self) -> None:
        block = self.workflow[self.workflow.index("  ci-gate:\n") :]
        self.assertNotIn("      - test-windows\n", block)
        self.assertNotIn("needs.test-windows.result", block)

    def test_palette_windows_is_required_for_changed_palette_on_prs(self) -> None:
        palette = job_block(self.workflow, "palette-windows", "rust-coverage")
        self.assertIn("if: ${{ needs.changes.outputs.palette == 'true' }}", palette)
        gate = self.workflow[self.workflow.index("  ci-gate:\n") :]
        self.assertIn("      - palette-windows\n", gate)
        self.assertIn('require_success_or_skipped palette-windows "${{ needs.palette-windows.result }}"', gate)

    def test_repository_workflows_use_hosted_runners(self) -> None:
        for path in WORKFLOW_DIR.glob("*.y*ml"):
            workflow = path.read_text(encoding="utf-8")
            self.assertNotIn("runs-on: ci-pool-", workflow, path)
            self.assertNotIn("runs-on: self-hosted", workflow, path)


if __name__ == "__main__":
    unittest.main()
