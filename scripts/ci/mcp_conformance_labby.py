#!/usr/bin/env python3
"""Drive official Skills scenarios through Labby's production boundaries."""

import argparse
import contextlib
import http.client
import http.server
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import threading
import time
import urllib.parse

MAX_BYTES = 8 * 1024 * 1024
META = {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "labby-conformance", "version": "1"},
    "io.modelcontextprotocol/clientCapabilities": {},
}
SERVER_SCENARIOS = (
    "tools-list", "caching", "sep-2640-skills-enumeration",
    "sep-2640-skills-manifest", "sep-2640-skills-directory",
)
CLIENT_SCENARIOS = (
    "sep-2640-client-no-prefetch", "sep-2640-client-verify-digest",
    "sep-2640-client-verify-size", "sep-2640-client-verify-frontmatter",
)


def relay_handler(port, token):
    class Relay(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass  # Never log request headers or credentials.

        def forward(self):
            if self.path != "/mcp":
                self.send_error(404)
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 <= length <= MAX_BYTES or self.headers.get("Transfer-Encoding"):
                    self.send_error(413)
                    return
                self.connection.settimeout(20)
                body = self.rfile.read(length)
                headers = {k: v for k, v in self.headers.items()
                           if k.lower() not in {"host", "connection", "authorization"}}
                headers["Authorization"] = "Bearer " + token
                with contextlib.closing(http.client.HTTPConnection("127.0.0.1", port, timeout=20)) as conn:
                    conn.request(self.command, "/mcp", body, headers)
                    response = conn.getresponse()
                    data = response.read(MAX_BYTES + 1)
                    if len(data) > MAX_BYTES:
                        self.send_error(502)
                        return
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() not in {"transfer-encoding", "content-length", "connection"}:
                            self.send_header(key, value)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
            except (OSError, ValueError, http.client.HTTPException):
                self.send_error(502, "Conformance relay failed")

        do_GET = do_POST = do_DELETE = forward
    return Relay


def require_evidence(output, scenario, server=False):
    prefix = "server-" if server else ""
    files = list(Path(output).glob(f"{prefix}{scenario}-*/checks.json"))
    if not files:
        raise RuntimeError(f"No evidence for {scenario}")
    checks = json.loads(max(files, key=lambda p: p.stat().st_mtime_ns).read_text())
    substantive = [c for c in checks if c["id"] != "wire-schema-valid"]
    if any(c["status"] == "FAILURE" for c in checks):
        raise RuntimeError(f"Failed assertions for {scenario}")
    # Exact substantive IDs from the pinned official runner. An unrelated
    # SUCCESS must never hide a skipped or missing target assertion.
    required = {
        "sep-2640-client-no-prefetch": {"sep-2640-host-no-prefetch"},
        "sep-2640-client-verify-digest": {"sep-2640-host-verify-digest"},
        "sep-2640-client-verify-size": {"sep-2640-host-size-mismatch-failure"},
        "sep-2640-client-verify-frontmatter": {"sep-2640-host-frontmatter-comparison"},
        "tools-list": {"tools-list", "tools-list-deterministic-order"},
        "caching": {"sep-2549-resources-read-caching-hints"},
        "sep-2640-skills-enumeration": {"sep-2640-skills-list-implemented", "sep-2640-skills-get-cache-attributes"},
        "sep-2640-skills-manifest": {"sep-2640-skillmd-mimetype", "sep-2640-final-segment-equals-name", "sep-2640-skillmd-metadata-name", "sep-2640-skillmd-metadata-description"},
        "sep-2640-skills-directory": {
            "sep-2640-capability-directory-read-flag", "sep-2640-directory-read-method-registered",
            "sep-2640-directory-read-result-resources-shape", "sep-2640-directory-read-subdir-mimetype",
            "sep-2640-directory-read-invalid-params", "sep-2640-directory-read-pagination",
        },
    }.get(scenario, set())
    successes = {c["id"] for c in substantive if c["status"] == "SUCCESS"}
    if not successes or not required <= successes:
        raise RuntimeError(f"Missing exercised assertions for {scenario}: {sorted(required - successes)}")
    if not server and scenario in CLIENT_SCENARIOS:
        stdout = max(files, key=lambda p: p.stat().st_mtime_ns).with_name("stdout.txt")
        if not stdout.is_file():
            raise RuntimeError(f"Missing production driver stdout for {scenario}")
        expected = "listed" if scenario.endswith("no-prefetch") else "verification-rejected"
        receipts = [json.loads(line) for line in stdout.read_text().splitlines() if line.startswith("{")]
        if not any(r.get("labbySkillsDriver") == scenario and r.get("outcome") == expected for r in receipts):
            raise RuntimeError(f"Missing production driver receipt for {scenario}")



def server_checks(args):
    # The upstream CLI has no authorization-header option. This task-local relay
    # adds the test credential while every request still crosses real middleware.
    token = os.environ["LABBY_CONFORMANCE_TOKEN"]
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), relay_handler(args.port, token)) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for scenario in SERVER_SCENARIOS:
                subprocess.run([
                    args.runner, "server", "--url", f"http://127.0.0.1:{server.server_port}/mcp",
                    "--scenario", scenario, "--spec-version", "2026-07-28",
                    "--timeout", "30000", "--force", "-o", args.output,
                ], check=True, timeout=45)
                require_evidence(args.output, scenario, server=True)
        finally:
            server.shutdown()
            thread.join(timeout=5)


class RpcError(RuntimeError):
    def __init__(self, method, error):
        super().__init__(f"{method}: {error}")
        self.error = error


class StdioPeer:
    def __init__(self, child):
        self.child = child
        self.pending = b""
        self.next_id = 0
        self.deadline = time.monotonic() + 22

    def request(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method,
                   "params": {**(params or {}), "_meta": META}}
        self.child.stdin.write(json.dumps(message).encode() + b"\n")
        self.child.stdin.flush()
        deadline = min(self.deadline, time.monotonic() + 20)
        while time.monotonic() < deadline:
            if b"\n" in self.pending:
                line, self.pending = self.pending.split(b"\n", 1)
                reply = json.loads(line)
                if reply.get("id") == self.next_id:
                    if "error" in reply:
                        raise RpcError(method, reply["error"])
                    return reply["result"]
                continue
            ready, _, _ = select.select([self.child.stdout], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                break
            chunk = os.read(self.child.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError("Labby closed its protocol stream")
            self.pending += chunk
            if len(self.pending) > MAX_BYTES:
                raise RuntimeError("Labby protocol response exceeded budget")
        raise TimeoutError(f"Labby {method} timed out")


def client_check(args):
    url = urllib.parse.urlsplit(args.url)
    if url.hostname not in {"127.0.0.1", "localhost"} or url.scheme != "http":
        raise ValueError("Official client fixture must be loopback HTTP")
    scenario = os.environ["MCP_CONFORMANCE_SCENARIO"]
    if scenario not in CLIENT_SCENARIOS:
        raise ValueError(f"Unsupported Skills scenario: {scenario}")
    with tempfile.TemporaryDirectory(prefix="labby-skills-conformance-") as scratch:
        home = Path(scratch)
        config = home / ".config/labby"
        config.mkdir(parents=True, mode=0o700)
        (config / "config.toml").write_text(
            '[code_mode]\nenabled = false\n[[upstream]]\nname = "fixture"\n'
            f'url = {json.dumps(args.url)}\nenabled = true\nproxy_skills = true\n'
        )
        env = {**os.environ, "HOME": str(home), "LABBY_HOME": str(config), "LABBY_LOG": "error",
               "LABBY_AUTH_MODE": "bearer", "LABBY_MCP_HTTP_TOKEN": "conformance-fixture-only",
               "LABBY_E2E_BOOTSTRAP_STATIC_OWNER": "1"}
        for key in ("LABBY_SERVER_URL", "CLAUDE_PLUGIN_OPTION_SERVER_URL"):
            env.pop(key, None)
        with (home / "stderr.log").open("wb") as stderr:
            child = subprocess.Popen([args.binary, "serve", "mcp", "--stdio"], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=stderr, env=env, cwd=home)
            try:
                peer = StdioPeer(child)
                peer.request("server/discover")
                reload_result = peer.request("tools/call", {"name": "gateway", "arguments": {
                    "action": "gateway.reload", "params": {}}})
                if reload_result.get("isError"):
                    raise RuntimeError("Labby gateway reload failed")
                listing = peer.request("skills/list")
                skill = next((s for s in listing["skills"]
                              if s["frontmatter"]["name"] == "pdf-processing"), None)
                if skill is None:
                    raise RuntimeError("Official fixture skill was not discovered through Labby")
                if scenario == "sep-2640-client-no-prefetch":
                    print(json.dumps({"labbySkillsDriver": scenario, "outcome": "listed"}))
                    return
                # Labby's shared provider verifies these bytes. The driver does
                # not implement verification or reinterpret a successful result.
                try:
                    peer.request("resources/read", {"uri": skill["uri"]})
                except RpcError as error:
                    kind = (error.error.get("data") or {}).get("kind")
                    if kind != "skill_digest_mismatch":
                        raise
                    print(json.dumps({"labbySkillsDriver": scenario,
                                      "outcome": "verification-rejected", "kind": kind}))
                    return
                supporting = next(r["uri"] for r in skill["resources"] if r["uri"] != skill["uri"])
                peer.request("resources/read", {"uri": supporting})
            finally:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
                child.stdin.close()
                child.stdout.close()


def main():
    def interrupted(_signum, _frame):
        raise SystemExit(130)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    server = sub.add_parser("server")
    server.add_argument("--port", type=int, required=True)
    server.add_argument("--runner", required=True)
    server.add_argument("--output", required=True)
    client = sub.add_parser("client")
    client.add_argument("--binary", required=True)
    client.add_argument("url")
    evidence = sub.add_parser("evidence")
    evidence.add_argument("--output", required=True)
    evidence.add_argument("--scenario", required=True)
    args = parser.parse_args()
    if args.mode == "server":
        server_checks(args)
    elif args.mode == "client":
        client_check(args)
    else:
        require_evidence(args.output, args.scenario)


if __name__ == "__main__":
    main()
