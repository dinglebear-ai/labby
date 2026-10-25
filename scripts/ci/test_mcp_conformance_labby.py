"""Behavioral checks for the local conformance auth bridge and evidence gate."""
import contextlib
import http.client
import http.server
import threading
import json
import tempfile
from pathlib import Path
import unittest

from scripts.ci.mcp_conformance_labby import relay_handler, require_evidence


@contextlib.contextmanager
def running(handler):
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            thread.join(timeout=3)


class ConformanceRelayTests(unittest.TestCase):
    def test_missing_or_skipped_client_evidence_cannot_pass(self):
        with tempfile.TemporaryDirectory() as output:
            with self.assertRaisesRegex(RuntimeError, "No evidence"):
                require_evidence(output, "sep-2640-client-verify-digest")
            path = Path(output) / "sep-2640-client-verify-digest-fixture"
            path.mkdir()
            checks = [{"id": "sep-2640-host-verify-digest", "status": "SKIPPED"}]
            (path / "checks.json").write_text(json.dumps(checks))
            with self.assertRaisesRegex(RuntimeError, "Missing exercised"):
                require_evidence(output, "sep-2640-client-verify-digest")
            checks[0]["status"] = "SUCCESS"
            (path / "checks.json").write_text(json.dumps(checks))
            with self.assertRaisesRegex(RuntimeError, "Missing production driver stdout"):
                require_evidence(output, "sep-2640-client-verify-digest")
            (path / "stdout.txt").write_text(json.dumps({"labbySkillsDriver": "sep-2640-client-verify-digest", "outcome": "verification-rejected"}) + "\n")
            require_evidence(output, "sep-2640-client-verify-digest")
            checks[0]["status"] = "FAILURE"
            (path / "checks.json").write_text(json.dumps(checks))
            with self.assertRaisesRegex(RuntimeError, "Failed assertions"):
                require_evidence(output, "sep-2640-client-verify-digest")

    def test_client_target_assertions_and_matching_receipts_are_required(self):
        scenarios = {
            "sep-2640-client-no-prefetch": ("sep-2640-host-no-prefetch", "listed"),
            "sep-2640-client-verify-digest": ("sep-2640-host-verify-digest", "verification-rejected"),
            "sep-2640-client-verify-size": ("sep-2640-host-size-mismatch-failure", "verification-rejected"),
            "sep-2640-client-verify-frontmatter": ("sep-2640-host-frontmatter-comparison", "verification-rejected"),
        }
        with tempfile.TemporaryDirectory() as output:
            for scenario, (target_id, outcome) in scenarios.items():
                path = Path(output) / f"{scenario}-fixture"
                path.mkdir()
                receipt = {"labbySkillsDriver": scenario, "outcome": outcome}
                (path / "stdout.txt").write_text(json.dumps(receipt) + "\n")
                unrelated = {"id": "client-initialized", "status": "SUCCESS"}
                for status in (None, "SKIPPED", "WARNING"):
                    with self.subTest(scenario=scenario, target_status=status):
                        checks = [unrelated]
                        if status is not None:
                            checks.append({"id": target_id, "status": status})
                        (path / "checks.json").write_text(json.dumps(checks))
                        with self.assertRaisesRegex(RuntimeError, "Missing exercised"):
                            require_evidence(output, scenario)
                (path / "checks.json").write_text(json.dumps([
                    unrelated, {"id": target_id, "status": "SUCCESS"}]))
                require_evidence(output, scenario)
                for invalid_receipt in ({}, {**receipt, "labbySkillsDriver": "other-scenario"},
                                        {**receipt, "outcome": "wrong-outcome"}):
                    with self.subTest(scenario=scenario, receipt=invalid_receipt):
                        (path / "stdout.txt").write_text(json.dumps(invalid_receipt) + "\n")
                        with self.assertRaisesRegex(RuntimeError, "Missing production driver receipt"):
                            require_evidence(output, scenario)

    def test_native_directory_support_must_execute_every_official_check(self):
        with tempfile.TemporaryDirectory() as output:
            path = Path(output) / "server-sep-2640-skills-directory-fixture"
            path.mkdir()
            checks = [{"id": "sep-2640-directory-read-method-registered", "status": "SKIPPED",
                       "errorMessage": "Server declared the skills extension but not directoryRead: true"}]
            (path / "checks.json").write_text(json.dumps(checks))
            with self.assertRaisesRegex(RuntimeError, "Missing exercised"):
                require_evidence(output, "sep-2640-skills-directory", server=True)
            checks = [{"id": name, "status": "SUCCESS"} for name in (
                "sep-2640-capability-directory-read-flag", "sep-2640-directory-read-method-registered",
                "sep-2640-directory-read-result-resources-shape", "sep-2640-directory-read-subdir-mimetype",
                "sep-2640-directory-read-invalid-params", "sep-2640-directory-read-pagination")]
            (path / "checks.json").write_text(json.dumps(checks))
            require_evidence(output, "sep-2640-skills-directory", server=True)

    def test_manifest_metadata_warnings_cannot_satisfy_native_evidence(self):
        with tempfile.TemporaryDirectory() as output:
            path = Path(output) / "server-sep-2640-skills-manifest-fixture"
            path.mkdir()
            checks = [{"id": "sep-2640-skillmd-mimetype", "status": "SUCCESS"},
                      {"id": "sep-2640-final-segment-equals-name", "status": "SUCCESS"},
                      {"id": "sep-2640-skillmd-metadata-name", "status": "WARNING"},
                      {"id": "sep-2640-skillmd-metadata-description", "status": "WARNING"}]
            (path / "checks.json").write_text(json.dumps(checks))
            with self.assertRaisesRegex(RuntimeError, "Missing exercised"):
                require_evidence(output, "sep-2640-skills-manifest", server=True)

    def test_injects_fixture_auth_and_preserves_protocol_and_denial(self):
        received = []

        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                received.append((self.path, dict(self.headers), self.rfile.read(2)))
                self.send_response(403)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"error":"denied"}')

        with running(Upstream) as upstream, running(relay_handler(upstream, "fixture-only")) as relay:
            with contextlib.closing(http.client.HTTPConnection("127.0.0.1", relay, timeout=3)) as conn:
                conn.request("POST", "/mcp", b"{}", {"Authorization": "Bearer ignored",
                             "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/list"})
                response = conn.getresponse()
                self.assertEqual(response.status, 403)
                self.assertEqual(response.read(), b'{"error":"denied"}')
        self.assertEqual(received[0][0], "/mcp")
        self.assertEqual(received[0][1]["Authorization"], "Bearer fixture-only")
        self.assertEqual(received[0][1]["MCP-Protocol-Version"], "2026-07-28")
        self.assertEqual(received[0][1]["Mcp-Method"], "tools/list")
        self.assertEqual(received[0][2], b"{}")

    def test_rejects_other_paths_and_unbounded_requests_before_forwarding(self):
        with running(relay_handler(1, "fixture-only")) as relay:
            for path, headers, expected in (("/other", {}, 404),
                                            ("/mcp", {"Content-Length": "99999999"}, 413),
                                            ("/mcp", {"Transfer-Encoding": "chunked"}, 413)):
                with self.subTest(path=path, headers=headers):
                    with contextlib.closing(http.client.HTTPConnection("127.0.0.1", relay, timeout=3)) as conn:
                        conn.request("POST", path, headers=headers)
                        response = conn.getresponse()
                        self.assertEqual(response.status, expected)
                        response.read()


if __name__ == "__main__":
    unittest.main()
