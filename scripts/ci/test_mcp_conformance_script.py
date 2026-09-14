import os
import subprocess
import time
import unittest
import tomllib

from scripts.ci.check_mcp_sdk_pin import matches_pin, matches_auth_pin
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class McpConformanceScriptTests(unittest.TestCase):
    @staticmethod
    def cleanup_helpers() -> str:
        script = (ROOT / "scripts/ci/mcp-conformance.sh").read_text()
        start = script.index("cleanup_initial_attempts=30")
        end = script.index("trap cleanup EXIT")
        return script[start:end]

    def run_cleanup_scenario(self, body: str) -> tuple[subprocess.CompletedProcess[str], float]:
        script = self.cleanup_helpers() + """
cleanup_initial_attempts=2
cleanup_term_attempts=2
cleanup_poll_interval=0.02
""" + body
        started = time.monotonic()
        completed = subprocess.run(
            ["bash", "-c", script],
            text=True,
            capture_output=True,
            timeout=2,
            check=False,
        )
        return completed, time.monotonic() - started

    def test_owned_process_cleanup_is_bounded_and_reports_escalation(self):
        graceful, graceful_elapsed = self.run_cleanup_scenario(
            """
sleep 60 &
pid=$!
stop_owned_process "$pid" TERM "graceful fixture"
"""
        )
        self.assertEqual(graceful.returncode, 0, graceful.stderr)
        self.assertLess(graceful_elapsed, 1)

        stubborn, stubborn_elapsed = self.run_cleanup_scenario(
            """
python3 -c 'import signal,time; signal.signal(signal.SIGINT, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)' &
pid=$!
sleep 0.05
stop_owned_process "$pid" INT "stubborn fixture"
"""
        )
        self.assertNotEqual(stubborn.returncode, 0)
        self.assertLess(stubborn_elapsed, 1)
        self.assertIn(
            "cleanup: stubborn fixture process", stubborn.stderr
        )
        self.assertIn("did not exit after INT; escalating", stubborn.stderr)

    def test_cleanup_preserves_primary_failure_and_promotes_cleanup_failure(self):
        graceful_trap, graceful_elapsed = self.run_cleanup_scenario(
            """
sleep 60 &
server_pid=$!
direct_proxy_pid=""
labby_pid=""
work_dir="$(mktemp -d)"
trap cleanup EXIT
exit 0
"""
        )
        self.assertEqual(graceful_trap.returncode, 0, graceful_trap.stderr)
        self.assertLess(graceful_elapsed, 1)

        stubborn_child = """
python3 -c 'import signal,time; signal.signal(signal.SIGINT, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)' &
direct_proxy_pid=$!
sleep 0.05
server_pid=""
labby_pid=""
work_dir="$(mktemp -d)"
"""
        promoted, _ = self.run_cleanup_scenario(
            stubborn_child + "trap cleanup EXIT\nexit 0"
        )
        self.assertEqual(promoted.returncode, 1, promoted.stderr)

        preserved, _ = self.run_cleanup_scenario(
            stubborn_child + "trap cleanup EXIT\nexit 7"
        )
        self.assertEqual(preserved.returncode, 7, preserved.stderr)

    def test_auth_alias_retains_same_immutable_client_only_sdk_pin(self):
        dependency = {"package": "rmcp", "git": "repo", "rev": "revision", "version": "=3.3.0", "default-features": False, "features": ["client", "auth", "transport-streamable-http-client-reqwest"], "optional": True}
        manifest = {"dependencies": {"rmcp-client": dependency}}
        self.assertTrue(matches_auth_pin(manifest, "repo", "revision", "=3.3.0"))
        for change in ({"rev": "other"}, {"version": "=3.1.4"}, {"path": "/temporary/sdk"}, {"default-features": True}, {"features": ["client", "auth", "server"]}, {"features": "client"}, {"features": [{}]}):
            with self.subTest(change=change):
                mutated = {"dependencies": {"rmcp-client": {**dependency, **change}}}
                self.assertFalse(matches_auth_pin(mutated, "repo", "revision", "=3.3.0"))
        for root_version in (None, "", "3.3", "=3.1.4"):
            self.assertFalse(matches_auth_pin(manifest, "repo", "revision", root_version))
        dependency.pop("version")
        self.assertFalse(matches_auth_pin(manifest, "repo", "revision", None))

    def test_default_conformance_pin_accepts_production_manifest(self):
        env = os.environ.copy()
        env.pop("LABBY_RMCP_REPOSITORY", None)
        env.pop("LABBY_RMCP_REVISION", None)
        completed = subprocess.run(
            ["bash", str(ROOT / "scripts/ci/mcp-conformance.sh"), "--check-sdk-pin"],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_conformance_pin_check_rejects_revision_mismatch(self):
        env = os.environ.copy()
        env.pop("LABBY_RMCP_REPOSITORY", None)
        env["LABBY_RMCP_REVISION"] = "0" * 40
        completed = subprocess.run(
            ["bash", str(ROOT / "scripts/ci/mcp-conformance.sh"), "--check-sdk-pin"],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(completed.returncode, 1)
        self.assertIn("immutable rmcp git revision", completed.stderr)

    def test_sdk_pin_is_semantic_not_dependent_on_toml_key_order(self):
        for declaration in [
            'rmcp = { git = "repo", rev = "revision", version = "=3.1.4" }',
            'rmcp = { version = "=3.1.4", rev = "revision", git = "repo" }',
            '[workspace.dependencies.rmcp]\nversion = "=3.1.4"\nrev = "revision"\ngit = "repo"',
        ]:
            with self.subTest(declaration=declaration):
                manifest = tomllib.loads('[workspace.dependencies]\n' + declaration)
                self.assertTrue(matches_pin(manifest, "repo", "revision"))

    def test_sdk_pin_rejects_wrong_source_or_revision_and_inactive_text(self):
        for declaration in [
            'rmcp = { git = "wrong", rev = "revision" }',
            'rmcp = { git = "repo", rev = "wrong" }',
            'rmcp = { git = "repo", branch = "revision" }',
            'rmcp = { git = "repo", rev = "revision", tag = "mutable" }',
            '# rmcp = { git = "repo", rev = "revision" }',
            'rmcp = "=3.1.4"',
        ]:
            with self.subTest(declaration=declaration):
                manifest = tomllib.loads('[workspace.dependencies]\n' + declaration)
                self.assertFalse(matches_pin(manifest, "repo", "revision"))

    def test_help_advertises_direct_proxy_only_mode(self) -> None:
        completed = subprocess.run(
            ["bash", str(ROOT / "scripts/ci/mcp-conformance.sh"), "--help"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            timeout=3,
        )

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("--direct-proxy-only", completed.stdout)
        self.assertIn("MCP_CONFORMANCE_OUTPUT_DIR", completed.stdout)
        self.assertIn("one-run development overrides", completed.stdout)

    def test_script_records_effective_pin_set_and_labels_overrides(self) -> None:
        script = (ROOT / "scripts/ci/mcp-conformance.sh").read_text()
        self.assertIn('"$output_dir/pins.json"', script)
        self.assertIn('"canonical": sys.argv[2] == "true"', script)
        self.assertIn("non-default pin overrides active; results are diagnostic", script)
        for field in (
            "labby_rmcp_repository",
            "labby_rmcp_revision",
            "rmcp_fixture_version",
            "rmcp_tag",
            "rmcp_commit",
            "mcp_conformance_version",
            "mcp_spec_version",
        ):
            self.assertIn(f'"{field}"', script)

    def test_authenticated_smoke_uses_dated_wire_metadata(self):
        script = (ROOT / "scripts/ci/mcp-conformance.sh").read_text()
        self.assertIn('"io.modelcontextprotocol/protocolVersion":"2026-07-28"', script)
        self.assertIn("--header 'MCP-Protocol-Version: 2026-07-28'", script)
        self.assertIn("--header 'Mcp-Method: tools/list'", script)
        self.assertNotIn("STATELESS=1", script)

    def test_script_honors_cargo_target_dir_and_absolute_output(self) -> None:
        script = (ROOT / "scripts/ci/mcp-conformance.sh").read_text()

        self.assertIn(
            'cargo_target_dir="${CARGO_TARGET_DIR:-${repo_root}/target}"', script
        )
        self.assertIn(
            'rmcp_target_dir="${CARGO_TARGET_DIR:-${work_dir}/rust-sdk/target}"',
            script,
        )
        self.assertIn('"${rmcp_target_dir}/debug/conformance-server"', script)
        self.assertIn('"${rmcp_target_dir}/debug/conformance-client"', script)
        self.assertIn('"${cargo_target_dir}/debug/labby" --json proxy', script)
        self.assertIn('"${cargo_target_dir}/debug/stdio-mcp-fixture"', script)
        self.assertIn('"${cargo_target_dir}/debug/labby" serve', script)
        self.assertIn('if [[ "$MCP_CONFORMANCE_OUTPUT_DIR" = /* ]]', script)
        self.assertIn(
            'tail -c 65536 "$error_file" >"${output_dir}/direct-proxy-readiness.stderr"',
            script,
        )
        self.assertIn('--max-time 10', script)
        self.assertIn('direct stdio proxy did not exit after Ctrl+C', script)
        self.assertIn('direct-proxy-shutdown.stderr', script)
        self.assertIn('stop_owned_process "$direct_proxy_pid" INT "direct proxy"', script)
        self.assertIn(
            'stop_owned_process "$server_pid" TERM "stock conformance server"', script
        )
        self.assertIn('stop_owned_process "$labby_pid" TERM "Labby server"', script)
        self.assertIn('kill -KILL "$pid"', script)
        self.assertIn('local status="$?"', script)
        self.assertIn('trap - EXIT', script)
        self.assertIn('exit "$status"', script)


if __name__ == "__main__":
    unittest.main()
