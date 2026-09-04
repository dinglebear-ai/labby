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
        self.assertIn("key: workspace-native-windows-v2", block)
        self.assertIn("cache-on-failure: true", block)
        self.assertIn("cargo test --workspace --all-features --locked --no-run", block)
        self.assertIn("Run maintained native Windows behavior suite", block)
        self.assertIn("pool::runner_handle", block)
        self.assertIn("dispatch::setup::provision", block)
        self.assertIn("parent_directory_sync_failure_is_visible", block)
        self.assertIn("shutdown_reaps_the_runner_and_its_descendant", block)
        self.assertIn("backup_from_permissive_parent_has_private_acl", block)
        self.assertIn("--test windows_job_object_reaping", block)
        self.assertIn("--run-ignored ignored-only", block)
        self.assertNotIn("--no-tests pass", block)

    def test_palette_windows_job_is_hosted_cached_and_bounded(self) -> None:
        block = job_block(self.workflow, "palette-windows", "rust-coverage")
        self.assertIn("runs-on: windows-latest", block)
        self.assertIn("timeout-minutes: 60", block)
        self.assertIn("Swatinem/rust-cache@", block)
        self.assertIn("key: palette-tauri-windows-v1", block)
        self.assertIn("cache-on-failure: true", block)

    def test_workspace_windows_job_is_required_and_palette_is_advisory(self) -> None:
        windows = job_block(self.workflow, "test-windows", "release-contract")
        self.assertIn("if: ${{ needs.changes.outputs.rust_test == 'true' }}", windows)
        block = self.workflow[self.workflow.index("  ci-gate:\n") :]
        self.assertIn("      - test-windows\n", block)
        self.assertNotIn("      - palette-windows\n", block)
        self.assertIn("needs.test-windows.result", block)
        self.assertNotIn("needs.palette-windows.result", block)

    def test_repository_workflows_use_hosted_runners(self) -> None:
        for path in WORKFLOW_DIR.glob("*.y*ml"):
            workflow = path.read_text(encoding="utf-8")
            self.assertNotIn("runs-on: ci-pool-", workflow, path)
            self.assertNotIn("runs-on: self-hosted", workflow, path)


if __name__ == "__main__":
    unittest.main()
