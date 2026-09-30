#!/usr/bin/env python3
"""Exercise Coco/Labby interop with private homes and loopback-only fixtures.

Requires explicit executable paths. Downloads nothing and never uses an existing
Labby home, endpoint, token or upstream. A fresh Linux install intentionally has
no owner: its Stash gate must be actionable, not misreported as an outage.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time


def fixture() -> int:
    """A legacy, prompt-only MCP provider with no filesystem/network actions."""
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-11-25",
                      "serverInfo": {"name": "coco-prompt-fixture", "version": "1"},
                      "capabilities": {"prompts": {}}}
        elif method == "prompts/list":
            result = {"prompts": [{"name": "greet", "description": "Test greeting",
                      "arguments": [{"name": "name", "required": True}]}]}
        elif method == "prompts/get":
            name = request.get("params", {}).get("arguments", {}).get("name", "unknown")
            result = {"messages": [{"role": "user", "content": {
                "type": "text", "text": "Hello " + name}}]}
        elif method == "ping":
            result = {}
        else:
            print(json.dumps({"jsonrpc": "2.0", "id": request["id"],
                              "error": {"code": -32601, "message": "Method not found"}}), flush=True)
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
    return 0


def stop(process: subprocess.Popen) -> None:
    """Terminate only the process group this harness created."""
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        process.wait(timeout=3)
        return
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=3)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--labby-bin", required=True, type=Path)
    parser.add_argument("--coco-bin", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    labby, coco = args.labby_bin.resolve(strict=True), args.coco_bin.resolve(strict=True)
    for path in (labby, coco):
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"Not an executable file: {path}")
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    rows = []
    token = ""
    daemon = None
    with tempfile.TemporaryDirectory(prefix="labby-coco-interop-") as tmp:
        root = Path(tmp).resolve()
        # No ambient LABBY_* or OAuth credentials may reach the fixture.
        env = {key: os.environ[key] for key in ("PATH", "LANG", "LC_ALL") if key in os.environ}
        env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
                   XDG_DATA_HOME=str(root / "data"), XDG_CACHE_HOME=str(root / "cache"),
                   TMPDIR=str(root), PYTHONUNBUFFERED="1")
        config = f'''config_version=1
[services]
built_in_upstream_apis_enabled=false
[code_mode]
enabled=true
timeout_ms=5000
[[upstream]]
name="audit-fixture"
command={json.dumps(sys.executable)}
args={json.dumps([str(Path(__file__).resolve()), "--fixture"])}
proxy_prompts=true
proxy_resources=false
'''
        stdio_home = root / "stdio"
        http_home = root / "http"
        for home in (stdio_home, http_home):
            (home / ".labby").mkdir(parents=True, mode=0o700)
            (home / ".labby/config.toml").write_text(config)
        stdio_env = dict(env, HOME=str(stdio_home))
        http_env = dict(env, HOME=str(http_home))

        def run(name, command, check, child_env):
            process = subprocess.Popen([str(coco), "--cli", "--color", "never", "--timeout", "5",
                                        *command], env=child_env, cwd=child_env["HOME"],
                                       text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       start_new_session=True)
            try:
                stdout, stderr = process.communicate(timeout=20)
                code = process.returncode
            except subprocess.TimeoutExpired:
                stop(process)
                stdout, stderr = process.communicate()
                code = 124
            except BaseException:
                stop(process)
                raise
            if token:
                stdout, stderr = stdout.replace(token, "<TEST_TOKEN>"), stderr.replace(token, "<TEST_TOKEN>")
            (output / (name + ".stdout")).write_text(stdout)
            (output / (name + ".stderr")).write_text(stderr)
            try:
                data = json.loads(stdout)
            except ValueError:
                data = None
            try:
                passed = bool(check(code, data, stderr))
            except (TypeError, KeyError, IndexError, AttributeError):
                passed = False
            rows.append({"test": name, "pass": passed, "exit": code})
            print(json.dumps(rows[-1]), flush=True)

        def launch_http(log_name):
            with (output / log_name).open("w") as log:
                process = subprocess.Popen([str(labby), "serve", "--host", "127.0.0.1", "--port", str(port)],
                                           env=http_env, cwd=http_home, stdout=log, stderr=subprocess.STDOUT,
                                           start_new_session=True)
            try:
                deadline = time.monotonic() + 25
                while True:
                    if process.poll() is not None:
                        raise RuntimeError("Isolated daemon exited; inspect " + log_name)
                    try:
                        with socket.create_connection(("127.0.0.1", port), 0.2):
                            return process
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise TimeoutError("Isolated daemon did not open its listener")
                        time.sleep(0.1)
            except BaseException:
                stop(process)
                raise

        try:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            daemon = launch_http("daemon.log")
            token = next(line.split("=", 1)[1].strip().strip(chr(34) + chr(39))
                         for line in (http_home / ".labby/.env").read_text().splitlines()
                         if line.startswith("LABBY_MCP_HTTP_TOKEN="))
            http_env["COCO_TEST_TOKEN"] = token
            for transport in ("stdio", "http"):
                child_env = stdio_env if transport == "stdio" else http_env
                for mode in ("legacy", "modern", "auto"):
                    prefix = transport + "-" + mode
                    if transport == "http" and mode != "legacy":
                        stop(daemon)
                        daemon = launch_http(prefix + "-daemon.log")
                    def command(action, extra):
                        if transport == "stdio":
                            return [action, "--protocol", mode, "stdio", *extra, "--", str(labby), "mcp"]
                        return [action, "--protocol", mode, "--bearer-env", "COCO_TEST_TOKEN",
                                f"http://127.0.0.1:{port}/mcp", *extra]
                    # First request, before any discovery or tool call.
                    run(prefix + "-cold-get", command("prompt", ["audit-fixture/greet", "--args", '{"name":"Ada"}']),
                        lambda rc, x, e: rc == 0 and x["messages"][0]["content"]["text"] == "Hello Ada", child_env)
                    run(prefix + "-missing-service", command("prompt", ["service-discover"]),
                        lambda rc, x, e: rc == 2 and "-32602" in e and "requires argument `service`" in e, child_env)
                    run(prefix + "-missing-action", command("prompt", ["run-action", "--args", '{"service":"gateway"}']),
                        lambda rc, x, e: rc == 2 and "-32602" in e and "requires argument `action`" in e, child_env)
                    run(prefix + "-valid-builtin", command("prompt", ["service-discover", "--args", '{"service":"gateway"}']),
                        lambda rc, x, e: rc == 0 and bool(x["messages"]), child_env)
                    run(prefix + "-code", command("call", ["codemode", "--args", json.dumps({
                        "code": "async () => { return {answer: 42}; }"})]),
                        lambda rc, x, e: rc == 0 and x["structuredContent"]["result"]["answer"] == 42, child_env)
                    run(prefix + "-catalog-read", command("read", ["lab://catalog"]),
                        lambda rc, x, e: rc == 0 and x["contents"][0]["uri"] == "lab://catalog", child_env)
                    def snapshot_check(rc, x, err):
                        if rc != 0 or not any(p["name"] == "audit-fixture/greet" for p in x["prompts"]):
                            return False
                        expected = "2025-11-25" if mode == "legacy" else "2026-07-28"
                        if x.get("protocolVersion") != expected:
                            return False
                        failures = x.get("listFailures", [])
                        if transport == "http" and sys.platform.startswith("linux"):
                            # The unprovisioned owner gate remains explicit and incomplete.
                            return (len(failures) == 1 and failures[0].get("method") == "resources/list"
                                    and "setup is required" in failures[0].get("error", ""))
                        return not failures
                    run(prefix + "-snapshot", command("snapshot", []), snapshot_check, child_env)
                    if transport == "http" and sys.platform.startswith("linux"):
                        run(prefix + "-stash-setup", command("read", ["stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV"]),
                            lambda rc, x, e: rc == 2 and "setup is required" in e, child_env)
            for mode in ("legacy", "modern", "auto"):
                run("http-" + mode + "-unauthenticated", ["snapshot", "--protocol", mode,
                    f"http://127.0.0.1:{port}/mcp"],
                    lambda rc, x, e: rc == 2 and "401" in e and "missing bearer token" in e, stdio_env)
        finally:
            if daemon is not None:
                stop(daemon)
            if token:
                for path in output.iterdir():
                    if path.is_file():
                        path.write_text(path.read_text().replace(token, "<TEST_TOKEN>"))
            summary = {"passed": sum(row["pass"] for row in rows), "total": len(rows),
                       "failures": [row["test"] for row in rows if not row["pass"]],
                       "platform": sys.platform,
                       "complete": len(rows) == (48 if sys.platform.startswith("linux") else 45),
                       "tests": rows}
            (output / "summary.json").write_text(json.dumps(summary, indent=2) + chr(10))
    print(json.dumps({k: v for k, v in summary.items() if k != "tests"}, indent=2))
    return int(bool(summary["failures"]) or not summary["complete"])


if __name__ == "__main__":
    if sys.argv[1:] == ["--fixture"]:
        raise SystemExit(fixture())
    raise SystemExit(main())
