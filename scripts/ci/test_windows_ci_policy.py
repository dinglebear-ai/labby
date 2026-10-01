from pathlib import Path
import ast
import os
import re
import subprocess
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

    def test_behavioral_recipe_collectors_provision_the_repository_task_runner(self) -> None:
        workflow = yaml.safe_load(self.workflow)
        for name in ("actionlint", "lifecycle-static-analysis"):
            with self.subTest(job=name):
                steps = workflow["jobs"][name]["steps"]
                collector = next(
                    i for i, step in enumerate(steps)
                    if "test_default_web_build.py" in step.get("run", "")
                    or "check-lifecycle-scripts.sh" in step.get("run", "")
                )
                provisions = [
                    step for step in steps[:collector]
                    if step.get("uses", "").startswith("jdx/mise-action@")
                    and "just" in step.get("with", {}).get("install_args", "").split()
                ]
                self.assertTrue(provisions, f"{name} runs real Justfile fixtures and needs pinned just")
                self.assertRegex((ROOT / ".mise.toml").read_text(), r'(?m)^just = "[0-9.]+"$')

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

    def test_only_advisory_desktop_windows_is_manually_selected(self) -> None:
        self.assertIn("      run_windows:\n", self.workflow)
        self.assertIn("        type: boolean\n", self.workflow)
        self.assertIn("        default: false\n", self.workflow)

        desktop = job_block(self.workflow, "desktop-windows", "verification-t0")
        manual_gate = "github.event_name == 'workflow_dispatch' && inputs.run_windows == true"
        self.assertIn(manual_gate, desktop)

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


def evaluate_condition(expression: str | None, context: dict) -> bool:
    """Evaluate the comparison/boolean subset used by the routed product jobs."""
    if expression is None:
        return True
    source = expression.strip().removeprefix("${{").removesuffix("}}").strip()
    token = re.compile(r"'[^']*'|[A-Za-z_][A-Za-z0-9_.-]*|&&|\|\||==|!=|[()]")
    converted = []
    cursor = 0
    for match in token.finditer(source):
        if source[cursor:match.start()].strip():
            raise ValueError(f"unsupported job condition: {source}")
        value = match.group()
        if value in ("&&", "||"):
            converted.append("and" if value == "&&" else "or")
        elif value in ("true", "false"):
            converted.append(str(value == "true"))
        elif value[0].isalpha():
            converted.append(repr(context.get(value, "")))
        else:
            converted.append(value)
        cursor = match.end()
    if source[cursor:].strip():
        raise ValueError(f"unsupported job condition: {source}")
    tree = ast.parse(" ".join(converted), mode="eval")
    allowed = (ast.Expression, ast.BoolOp, ast.Compare, ast.Constant, ast.And, ast.Or, ast.Eq, ast.NotEq)
    if any(not isinstance(node, allowed) for node in ast.walk(tree)):
        raise ValueError("unsupported expression node")
    return bool(eval(compile(tree, "<workflow condition>", "eval"), {"__builtins__": {}}, {}))


class ProductRoutingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.jobs = yaml.safe_load(WORKFLOW.read_text())["jobs"]

    def context(self, event: str, fork: bool, files: list[str]) -> dict:
        from scripts.ci.changed_paths import classify
        output = classify(event, files)
        context = {
            "github.event_name": event,
            "github.repository": "acme/labby",
            "github.event.pull_request.head.repo.full_name": "contributor/labby" if fork else "acme/labby",
            "inputs.run_windows": False,
        }
        context.update({"needs.changes.outputs." + key: str(value).lower() for key, value in output.items()})
        return context

    def run_aggregate(self, context: dict, **results: str) -> subprocess.CompletedProcess:
        gate = self.jobs["ci-gate"]
        context = {**context, **{"needs." + job + ".result": results.get(job, "success") for job in gate["needs"]}}
        step = next(step for step in gate["steps"] if step["name"] == "verify required jobs passed or were intentionally skipped")
        interpolate = lambda text: re.sub(r"\$\{\{\s*([^{}]+?)\s*\}\}", lambda match: str(context.get(match[1].strip(), "")), text)
        return subprocess.run(
            ["bash", "-c", interpolate(step["run"])],
            env={**os.environ, **{key: interpolate(value) for key, value in step.get("env", {}).items()}},
            text=True, capture_output=True, timeout=10, check=False,
        )

    def test_native_windows_executes_on_routed_events(self) -> None:
        for event, fork in [("pull_request", False), ("pull_request", True), ("push", False), ("schedule", False), ("workflow_dispatch", False)]:
            with self.subTest(event=event, fork=fork):
                context = self.context(event, fork, ["crates/labby-gateway/src/upstream/process_guard.rs"])
                self.assertTrue(evaluate_condition(self.jobs["test-windows"].get("if"), context))

    def test_installer_executes_on_routed_events(self) -> None:
        for event in ("pull_request", "push", "workflow_dispatch"):
            with self.subTest(event=event):
                context = self.context(event, False, ["scripts/install.ps1"])
                self.assertTrue(evaluate_condition(self.jobs["windows-installer"].get("if"), context))

    def test_required_windows_skip_fails_aggregate(self) -> None:
        context = self.context("pull_request", False, ["crates/labby-gateway/src/upstream/process_guard.rs"])
        result = self.run_aggregate(context, **{"test-windows": "skipped"})
        self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)

    def test_fork_product_routing_has_a_classifier_and_test_lane(self) -> None:
        for files, expected in [(["crates/labby-gateway/src/gateway/dispatch.rs"], "test-fork"), (["apps/web/app/(admin)/gateway/page.tsx"], "gateway-admin-browser")]:
            with self.subTest(files=files):
                context = self.context("pull_request", True, files)
                self.assertTrue(evaluate_condition(self.jobs["changes"].get("if"), context))
                self.assertTrue(evaluate_condition(self.jobs[expected].get("if"), context))
                result = self.run_aggregate(context, **{expected: "skipped"})
                self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)

    def test_classifier_skip_fails_even_on_a_fork(self) -> None:
        result = self.run_aggregate(self.context("pull_request", True, ["README.md"]), changes="skipped")
        self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)

    def test_unrouted_windows_skip_is_intentional(self) -> None:
        context = self.context("pull_request", False, ["docs/services/ACCESS.md"])
        self.assertFalse(evaluate_condition(self.jobs["test-windows"].get("if"), context))
        result = self.run_aggregate(context, **{"test-windows": "skipped", "windows-installer": "skipped", "test-fork": "skipped", "test": "skipped"})
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
