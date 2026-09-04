import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("mcp_upstream_drift.py")
SPEC = importlib.util.spec_from_file_location("mcp_upstream_drift", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class OwnershipMappingTest(unittest.TestCase):
    def test_auth_and_transport_changes_map_to_concrete_code_and_checks(self):
        paths, checks = MODULE.map_ownership(
            [
                "docs/specification/draft/basic/authorization/index.mdx",
                "crates/rmcp/src/transport/streamable_http.rs",
            ]
        )
        self.assertIn("crates/labby-auth/src/", paths)
        self.assertIn("crates/labby/src/cli/serve.rs", paths)
        self.assertIn("cargo test -p labby-auth --all-features --locked", checks)

    def test_unknown_changes_still_map_to_baseline_and_conformance_owners(self):
        paths, checks = MODULE.map_ownership(["README.md"])
        self.assertIn("Cargo.toml", paths)
        self.assertIn("scripts/ci/mcp-conformance.sh", checks)

    def test_workflow_uses_python3_on_ops_runners(self):
        workflow = (
            Path(__file__).parents[2] / ".github/workflows/mcp-upstream-drift.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "python3 -m unittest scripts/ci/test_mcp_upstream_drift.py", workflow
        )
        self.assertIn("python3 scripts/ci/mcp_upstream_drift.py", workflow)
        self.assertNotIn("run: python -m unittest", workflow)
        self.assertNotIn("\n          python scripts/ci/mcp_upstream_drift.py", workflow)

    def test_main_ci_runs_accepted_skills_contract_and_feature_slice(self):
        workflow = (
            Path(__file__).parents[2] / ".github/workflows/ci.yml"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "slice: [gateway, gateway-host, integrated-gateway, fs, skills]",
            workflow,
        )
        self.assertIn("Accepted SEP-2640 server, client, and intermediary behavior", workflow)
        self.assertIn("--no-default-features --features skills", workflow)


if __name__ == "__main__":
    unittest.main()
