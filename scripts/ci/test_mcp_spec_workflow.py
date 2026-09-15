"""Enforce the source-bound MCP specification oracle workflow contract."""

from pathlib import Path
import re
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[2]


class McpSpecWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = yaml.load(
            (ROOT / ".github/workflows/ci.yml").read_text(),
            Loader=yaml.BaseLoader,
        )
        cls.steps = cls.workflow["jobs"]["mcp-conformance"]["steps"]

    def step(self, prefix: str) -> dict:
        return next(step for step in self.steps if step.get("name", "").startswith(prefix))

    def test_source_checkout_is_immutable_and_credentialless(self) -> None:
        checkout = self.step("Checkout immutable MCP specification")
        self.assertEqual(
            checkout["with"]["repository"],
            "modelcontextprotocol/modelcontextprotocol",
        )
        self.assertRegex(checkout["with"]["ref"], re.compile(r"^[0-9a-f]{40}$"))
        self.assertEqual(checkout["with"]["path"], "target/mcp-spec-source")
        self.assertEqual(checkout["with"]["persist-credentials"], "false")

    def test_locked_fetch_precedes_offline_dependency_binding(self) -> None:
        fetch = self.step("Fetch locked dependencies")
        run = self.step("Run registered specification oracles")
        self.assertEqual(fetch["run"], "cargo fetch --locked")
        self.assertLess(self.steps.index(fetch), self.steps.index(run))
        reporter = (ROOT / "scripts/ci/mcp_spec_compliance.py").read_text()
        self.assertIn(
            '["cargo", "metadata", "--locked", "--offline", "--all-features",',
            reporter,
        )

    def test_registered_oracle_gate_is_not_a_full_compliance_claim(self) -> None:
        run = self.step("Run registered specification oracles")
        self.assertIn("not a full-compliance claim", run["name"])
        self.assertIn("run --gate oracles --spec-checkout", run["run"])
        self.assertNotIn("--gate full", run["run"])

    def test_source_recipes_are_exercised_with_installed_just(self) -> None:
        validation = self.step("Validate full specification inventory")
        self.assertIn("scripts.ci.test_mcp_spec_recipes", validation["run"])
        setup = next(step for step in self.steps if step.get("uses", "").startswith("jdx/mise-action@"))
        self.assertIn("just", setup["with"]["install_args"].split())
        self.assertLess(self.steps.index(setup), self.steps.index(validation))

    def test_receipts_and_coverage_gaps_are_always_retained(self) -> None:
        upload = self.step("Preserve specification coverage gaps")
        self.assertEqual(upload["if"], "always()")
        self.assertEqual(upload["with"]["name"], "mcp-spec-compliance")
        self.assertEqual(upload["with"]["path"], "target/mcp-spec-compliance")
        self.assertEqual(upload["with"]["if-no-files-found"], "warn")

    def test_workflow_policy_job_runs_this_contract(self) -> None:
        policy = self.workflow["jobs"]["fleet-policy"]["steps"]
        command = "\n".join(step.get("run", "") for step in policy)
        self.assertIn("scripts/ci/test_mcp_spec_workflow.py", command)


if __name__ == "__main__":
    unittest.main()
