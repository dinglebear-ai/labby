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


if __name__ == "__main__":
    unittest.main()
