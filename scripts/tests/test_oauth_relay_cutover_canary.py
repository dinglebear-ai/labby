from __future__ import annotations

import http.server
import importlib.util
import json
import pathlib
import threading
import urllib.request
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "oauth_relay_cutover_canary.py"
SPEC = importlib.util.spec_from_file_location("oauth_relay_cutover_canary", SCRIPT)
assert SPEC and SPEC.loader
canary = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(canary)


class FakeRelay(http.server.BaseHTTPRequestHandler):
    registry: dict[str, str] = {}
    fail_registration = False
    fail_callback = False
    target_unreachable = False
    corrupt_response = False
    fail_cleanup = False
    retain_residual = False

    def do_PUT(self) -> None:  # noqa: N802
        if self.fail_registration:
            self.reply(503, {})
            return
        machine = self.path.rsplit("/", 1)[-1]
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.registry[machine] = payload["target_url"]
        self.reply(200, {})

    def do_DELETE(self) -> None:  # noqa: N802
        if self.fail_cleanup:
            self.reply(503, {})
            return
        machine = self.path.rsplit("/", 1)[-1]
        if not self.retain_residual:
            self.registry.pop(machine, None)
        self.reply(200, {})

    def do_GET(self) -> None:  # noqa: N802
        if "/v1/oauth/relay/machines/" in self.path:
            machine = self.path.rsplit("/", 1)[-1]
            self.reply(200 if machine in self.registry else 404, {})
            return
        if self.fail_callback:
            self.reply(502, {})
            return
        if self.target_unreachable:
            self.reply(504, {})
            return
        machine = self.path.split("/callback/", 1)[1].split("?", 1)[0]
        target = self.registry[machine] + "?" + self.path.split("?", 1)[1]
        with urllib.request.urlopen(target, timeout=2) as response:
            body = (
                b'{"canary":"oauth-relay","ok":false}'
                if self.corrupt_response
                else response.read()
            )
            self.send_response(response.status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def reply(self, status: int, body: object) -> None:
        encoded = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class CanaryTests(unittest.TestCase):
    def setUp(self) -> None:
        FakeRelay.registry = {}
        FakeRelay.fail_registration = False
        FakeRelay.fail_callback = False
        FakeRelay.target_unreachable = False
        FakeRelay.corrupt_response = False
        FakeRelay.fail_cleanup = False
        FakeRelay.retain_residual = False
        self.relay = canary.ThreadingServer(("127.0.0.1", 0), FakeRelay)
        self.thread = threading.Thread(target=self.relay.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self) -> None:
        self.relay.shutdown()
        self.relay.server_close()
        self.thread.join(timeout=2)

    def args(self) -> object:
        target = canary.ThreadingServer(("127.0.0.1", 0), http.server.BaseHTTPRequestHandler)
        port = target.server_port
        target.server_close()
        return type("Args", (), {
            "phase": "post-cutover",
            "public_base": f"http://127.0.0.1:{self.relay.server_port}",
            "admin_base": f"http://127.0.0.1:{self.relay.server_port}",
            "target_host": "127.0.0.1",
            "bind_host": "127.0.0.1",
            "target_port": port,
            "admin_token_env": "TEST_ADMIN_TOKEN",
            "timeout": 2.0,
        })()

    def test_exact_callback_and_cleanup_pass(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        result = canary.run(self.args())
        self.assertEqual(result["status"], "passed")
        self.assertEqual(FakeRelay.registry, {})

    def test_proxy_routing_failure_blocks_transition_and_still_cleans(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.fail_callback = True
        with self.assertRaisesRegex(RuntimeError, "public callback canary failed"):
            canary.run(self.args())
        self.assertEqual(FakeRelay.registry, {})

    def test_target_unreachable_blocks_transition_and_still_cleans(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.target_unreachable = True
        with self.assertRaisesRegex(RuntimeError, "public callback canary failed"):
            canary.run(self.args())
        self.assertEqual(FakeRelay.registry, {})

    def test_registry_failure_blocks_transition_without_residue(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.fail_registration = True
        with self.assertRaisesRegex(RuntimeError, "registration failed"):
            canary.run(self.args())
        self.assertEqual(FakeRelay.registry, {})

    def test_response_corruption_blocks_transition_and_still_cleans(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.corrupt_response = True
        with self.assertRaisesRegex(RuntimeError, "public callback canary failed"):
            canary.run(self.args())
        self.assertEqual(FakeRelay.registry, {})

    def test_cleanup_failure_overrides_success(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.fail_cleanup = True
        with self.assertRaisesRegex(RuntimeError, "cleanup/residual audit failed"):
            canary.run(self.args())
        self.assertEqual(len(FakeRelay.registry), 1)

    def test_positive_residual_audit_overrides_success(self) -> None:
        import os
        os.environ["TEST_ADMIN_TOKEN"] = "sentinel-admin-token"
        FakeRelay.retain_residual = True
        with self.assertRaisesRegex(RuntimeError, "residual audit returned"):
            canary.run(self.args())
        self.assertEqual(len(FakeRelay.registry), 1)

    def test_admin_base_rejects_insecure_remote_and_lookalike_loopback(self) -> None:
        for value in (
            "http://example.com",
            "http://localhost.example.com",
            "http://user@localhost",
            "https://example.com?query=1",
        ):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                canary.validate_admin_base(value)
        self.assertEqual(canary.validate_admin_base("http://127.0.0.1:8080"), "http://127.0.0.1:8080")
        self.assertEqual(canary.validate_admin_base("https://example.com/"), "https://example.com")

    def test_request_json_never_follows_redirect_or_forwards_bearer(self) -> None:
        captured: list[str | None] = []

        class Capture(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                captured.append(self.headers.get("Authorization"))
                self.send_response(200)
                self.end_headers()

            def log_message(self, _format: str, *_args: object) -> None:
                return

        target = canary.ThreadingServer(("127.0.0.1", 0), Capture)
        target_thread = threading.Thread(target=target.serve_forever, daemon=True)
        target_thread.start()

        class Redirect(http.server.BaseHTTPRequestHandler):
            status = 302

            def do_GET(self) -> None:  # noqa: N802
                self.send_response(self.status)
                self.send_header("Location", f"http://127.0.0.1:{target.server_port}/capture")
                self.end_headers()

            def log_message(self, _format: str, *_args: object) -> None:
                return

        origin = canary.ThreadingServer(("127.0.0.1", 0), Redirect)
        origin_thread = threading.Thread(target=origin.serve_forever, daemon=True)
        origin_thread.start()
        try:
            for status in (301, 302, 303, 307, 308):
                Redirect.status = status
                code, _ = canary.request_json(
                    f"http://127.0.0.1:{origin.server_port}/admin",
                    token="sentinel-admin-token",
                )
                self.assertEqual(code, status)
            self.assertEqual(captured, [])
        finally:
            origin.shutdown()
            origin.server_close()
            origin_thread.join(timeout=2)
            target.shutdown()
            target.server_close()
            target_thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
