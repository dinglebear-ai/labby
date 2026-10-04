#!/usr/bin/env python3
"""Supported native bearer first-use driver; real operations only, disposable images only."""
from __future__ import annotations

import argparse
import http.cookiejar
import importlib.util
import json
import os
import math
import re
import signal
import tempfile
from pathlib import Path
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

spec = importlib.util.spec_from_file_location("qualification", Path(__file__).with_name("qualify-first-use.py"))
qualification = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualification)


def required(name: str) -> str:
    value = os.environ.get(name, "")
    if not value.strip():
        raise qualification.QualificationError(f"required runner input {name} is missing")
    return value


def write_private(path: Path, value: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value)


class Gateway:
    def __init__(self):
        self.base = required("LABBY_QUALIFICATION_GATEWAY_URL").rstrip("/")
        self.token = qualification.read_private_token(Path(required("LABBY_QUALIFICATION_TOKEN_FILE")))
        self.csrf = None
        self.opener = urllib.request.build_opener(qualification.NoRedirect(), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def request(self, path: str, body=None):
        headers = {"Authorization": "Bearer " + self.token, "Content-Type": "application/json", "Origin": self.base}
        if self.csrf:
            headers["X-CSRF-Token"] = self.csrf
        request = urllib.request.Request(self.base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
        try:
            with self.opener.open(request, timeout=45) as response:
                raw = response.read(2 * 1024 * 1024 + 1)
                if len(raw) > 2 * 1024 * 1024:
                    raise qualification.QualificationError("gateway response exceeded driver limit")
                return json.loads(raw)
        except urllib.error.HTTPError as error:
            raise qualification.QualificationError(f"gateway operation failed with HTTP {error.code}") from None

    def action(self, service: str, action: str, params: dict):
        return self.request("/v1/" + service, {"action": action, "params": params})

    def principal(self):
        self.request("/auth/bearer-session", {})
        session = self.request("/auth/session")
        self.csrf = session.get("csrf_token")
        owner = session.get("owner", {})
        if session.get("authority_state") != "ready" or owner.get("kind") != "personal" or not owner.get("id"):
            raise qualification.QualificationError("current authenticated personal ownership is not ready")
        return owner["id"]


def bootstrap():
    binary = required("LABBY_QUALIFICATION_BINARY")
    subprocess.run([binary, "setup", "--yes", "--role", "server", "--deployment", "native", "--no-desktop", "--no-browser"], check=True)
    # Read only the generated invoking-user CLI credential. Configuration is
    # produced by binary-owned setup, never edited by this driver.
    env_path = Path.home() / ".labby" / ".env"
    descriptor = os.open(env_path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        raw = os.read(descriptor, 1024 * 1024 + 1).decode()
    finally:
        os.close(descriptor)
    if len(raw) > 1024 * 1024:
        raise qualification.QualificationError("generated client environment exceeds driver limit")
    token = None
    for line in raw.splitlines():
        if line.startswith("LABBY_MCP_HTTP_TOKEN="):
            values = shlex.split(line.partition("=")[2], comments=True)
            if len(values) == 1:
                token = values[0]
    if not token:
        raise qualification.QualificationError("native setup did not produce a bearer client credential")
    write_private(Path(required("LABBY_QUALIFICATION_TOKEN_FILE")), token)
    gateway = Gateway()
    deadline = time.monotonic() + 45
    while True:
        try:
            gateway.principal()
            gateway.action("setup", "readiness.state", {})
            return
        except (OSError, qualification.QualificationError):
            if time.monotonic() >= deadline:
                raise qualification.QualificationError("native authenticated bootstrap did not become ready") from None
            time.sleep(0.25)


def agent(gateway: Gateway):
    owner = gateway.principal()
    provider_url = required("LABBY_QUALIFICATION_PROVIDER_URL")
    parsed = urllib.parse.urlsplit(provider_url)
    if not parsed.hostname or parsed.scheme not in {"https", "http"} or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise qualification.QualificationError("provider URL is invalid")
    if parsed.scheme == "http" and parsed.hostname not in {"localhost", "127.0.0.1", "::1"}:
        raise qualification.QualificationError("qualification provider requires HTTPS or loopback")
    protocol = os.environ.get("LABBY_QUALIFICATION_PROVIDER_PROTOCOL", "openai")
    if protocol not in {"openai", "phoenix"}:
        raise qualification.QualificationError("provider protocol must be openai or phoenix")
    gateway.action("setup", "settings.env.update", {"section": "agents", "confirm": True, "entries": [
        {"key": "LABBY_AGENT_PROVIDER_PROTOCOL", "value": protocol},
        {"key": "LABBY_PHOENIX_OPENAI_BASE_URL", "value": provider_url},
        {"key": "LABBY_PHOENIX_OPENAI_API_KEY", "value": os.environ.get("LABBY_QUALIFICATION_PROVIDER_KEY", "")},
    ]})
    models = gateway.action("agents", "agents.models.list", {"owner_kind": "personal", "owner_id": owner})
    model = required("LABBY_QUALIFICATION_MODEL")
    if model not in models.get("models", []):
        raise qualification.QualificationError("selected model is not offered by the connected provider")
    identifier = "first-use-qualification"
    gateway.action("agents", "agents.create", {"agent_id": identifier, "owner_kind": "personal", "owner_id": owner,
        "model": model, "instructions": "Respond concisely to the user. This is a bounded first-use connection test."})
    result = gateway.action("agents", "agents.run", {"agent_id": identifier, "input": "Reply with one short sentence confirming that you can respond."})
    if result.get("status") != "completed" or not isinstance(result.get("output"), str) or not result["output"].strip():
        raise qualification.QualificationError("starter Agent did not complete with real text output")


def clients(gateway: Gateway):
    selected = json.loads(required("LABBY_QUALIFICATION_SELECTED_CLIENTS"))
    if selected:
        if not isinstance(selected, list) or any(client not in {"codex", "claude-code"} for client in selected):
            raise qualification.QualificationError("invalid selected-client list")
        subprocess.run([required("LABBY_QUALIFICATION_BINARY"), "setup", "clients", "connect", "--clients", ",".join(selected)], check=True)
        return
    gateway.action("setup", "readiness.clients.defer", {})


def discover(gateway: Gateway):
    artifact_id = required("LABBY_QUALIFICATION_ARTIFACT_ID")
    revision_id = required("LABBY_QUALIFICATION_REVISION_ID")
    expected_url = required("LABBY_QUALIFICATION_MCP_URL")
    page = gateway.request("/v1/depot/discover", {"provider": "public", "query": required("LABBY_QUALIFICATION_QUERY"), "kind": "mcp-server", "limit": 50})
    if page.get("state") != "complete" or page.get("coverageComplete") is not True or page.get("failures") or not page.get("items"):
        raise qualification.QualificationError("default public catalog did not return complete real results")
    matches = [artifact for artifact in page["items"] if artifact.get("providerId") == "public" and artifact.get("artifactId") == artifact_id]
    if len(matches) != 1:
        raise qualification.QualificationError("approved catalog artifact was not found exactly once")
    artifact = matches[0]
    metadata = artifact.get("mcpConnection", {})
    current = artifact.get("currentRevisionId") or artifact.get("currentRevision", {}).get("id")
    if metadata.get("schemaVersion") != "labby.mcp-connection/v1" or metadata.get("transport") != "http" or metadata.get("authentication") not in {"none", "bearer"} or current != revision_id or metadata.get("revisionId") != revision_id or metadata.get("url") != expected_url:
        raise qualification.QualificationError("approved revision has no matching supported installation metadata")
    parsed = urllib.parse.urlsplit(expected_url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise qualification.QualificationError("approved MCP metadata endpoint is invalid")
    write_private(Path(required("LABBY_QUALIFICATION_TOKEN_FILE") + ".artifact.json"), json.dumps(metadata))


def advertised_tools(gateway: Gateway) -> list[dict]:
    session = None
    protocol = "2025-03-26"
    def request(method, params, identifier):
        nonlocal session, protocol
        body = {"jsonrpc": "2.0", "method": method, "params": params}
        if identifier is not None:
            body["id"] = identifier
        headers = {"Authorization": "Bearer " + gateway.token, "Content-Type": "application/json", "Accept": "application/json, text/event-stream", "MCP-Protocol-Version": protocol}
        if session:
            headers["Mcp-Session-Id"] = session
        with gateway.opener.open(urllib.request.Request(gateway.base + "/mcp", data=json.dumps(body).encode(), headers=headers), timeout=15) as response:
            session = response.headers.get("Mcp-Session-Id") or session
            raw = response.read(2 * 1024 * 1024 + 1)
            if len(raw) > 2 * 1024 * 1024:
                raise qualification.QualificationError("MCP discovery response exceeds driver limit")
            if identifier is None:
                return {}
            if response.headers.get("Content-Type", "").startswith("text/event-stream"):
                candidates = [json.loads(line[5:].strip()) for line in raw.decode().splitlines() if line.startswith("data:")]
                matches = [value for value in candidates if value.get("id") == identifier]
                if len(matches) != 1:
                    raise qualification.QualificationError("MCP discovery has no unique response")
                value = matches[0]
            else:
                value = json.loads(raw)
            if value.get("id") != identifier or "error" in value:
                raise qualification.QualificationError("MCP discovery failed")
            return value["result"]
    try:
        initialized = request("initialize", {"protocolVersion": protocol, "capabilities": {}, "clientInfo": {"name": "labby-release-qualification", "version": "1"}}, 1)
        protocol = initialized["protocolVersion"]
        request("notifications/initialized", {}, None)
        tools, cursor = [], None
        for identifier in range(2, 12):
            result = request("tools/list", {"cursor": cursor} if cursor else {}, identifier)
            tools.extend(result["tools"])
            cursor = result.get("nextCursor")
            if not cursor:
                return tools
        raise qualification.QualificationError("MCP tool discovery exceeded its bounded page limit")
    finally:
        if session:
            try:
                gateway.opener.open(urllib.request.Request(gateway.base + "/mcp", method="DELETE", headers={"Authorization": "Bearer " + gateway.token, "Mcp-Session-Id": session, "MCP-Protocol-Version": protocol}), timeout=5).close()
            except Exception:
                pass


def run_selected_client(argv: list[str], directory: str, timeout: float = 60) -> None:
    process = subprocess.Popen(argv, cwd=directory, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, start_new_session=True)
    def interrupted(_signal, _frame):
        raise qualification.QualificationError('selected client stage was terminated')
    previous_handler = signal.signal(signal.SIGTERM, interrupted)
    try:
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            raise qualification.QualificationError('selected client timed out; its process group was stopped') from None
    finally:
        # The outer harness first sends SIGTERM, allowing cleanup of this
        # separate client group before it forcibly stops a stuck stage.
        signal.signal(signal.SIGTERM, previous_handler)
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
    if code:
        raise qualification.QualificationError(f'selected client exited with status {code}')


def external_clients(gateway: Gateway, tool: str, arguments: dict):
    selected = json.loads(required("LABBY_QUALIFICATION_SELECTED_CLIENTS"))
    if not selected:
        return
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", tool) or any(isinstance(value, (dict, list)) or isinstance(value, float) and not math.isfinite(value) for value in arguments.values()):
        raise qualification.QualificationError("external-client qualification requires a simple named tool and scalar arguments")
    matches = [entry for entry in advertised_tools(gateway) if entry.get("name") == tool]
    if len(matches) != 1 or matches[0].get("annotations", {}).get("readOnlyHint") is not True or matches[0].get("annotations", {}).get("destructiveHint") is True:
        raise qualification.QualificationError("approved tool is not uniquely advertised with safe read-only annotations")
    prompt = "Use only the configured MCP server lab. Call its approved read-only tool " + tool + " exactly once with arguments " + json.dumps(arguments, allow_nan=False) + ". Do not access files, run commands, browse, change settings, or call other tools. Return the tool result briefly. If this tool cannot be called, stop and report failure."
    with tempfile.TemporaryDirectory(prefix="labby-client-qualification-") as directory:
        for client in selected:
            if client == "codex":
                model = required("LABBY_QUALIFICATION_CODEX_MODEL")
                argv = ["codex", "exec", "--sandbox", "read-only", "--strict-config", "--ephemeral", "--skip-git-repo-check", "--cd", directory,
                        "--disable", "shell_tool", "--disable", "unified_exec", "--model", model,
                        "--config", 'approval_policy="never"', "--config", 'web_search="disabled"',
                        "--config", "mcp_servers.lab.enabled_tools=" + json.dumps([tool]), prompt]
                inventory = subprocess.run(["codex", "mcp", "list", "--json"], capture_output=True, text=True, timeout=10, check=True)
                try:
                    servers = json.loads(inventory.stdout)
                except (ValueError, TypeError):
                    raise qualification.QualificationError("selected client MCP inventory is invalid") from None
                if not isinstance(servers, list) or any(not isinstance(entry, dict) or not isinstance(entry.get("name"), str) for entry in servers) or not any(entry["name"] == "lab" for entry in servers):
                    raise qualification.QualificationError("selected client MCP inventory does not contain the registered Labby server")
                for entry in servers:
                    if entry["name"] != "lab":
                        argv[-1:-1] = ["--config", "mcp_servers." + json.dumps(entry["name"]) + ".enabled=false"]
                help_command, flags = ["codex", "exec", "--help"], ["--sandbox", "--strict-config", "--ephemeral", "--skip-git-repo-check", "--model", "--config"]
            else:
                model = required("LABBY_QUALIFICATION_CLAUDE_MODEL")
                argv = ["claude", "--print", "--model", model, "--tools", "", "--allowedTools", "mcp__lab__" + tool,
                        "--permission-mode", "dontAsk", "--no-session-persistence", "--max-budget-usd", "1", prompt]
                help_command, flags = ["claude", "--help"], ["--print", "--model", "--tools", "--allowedTools", "--permission-mode", "--no-session-persistence", "--max-budget-usd"]
            help_result = subprocess.run(help_command, capture_output=True, text=True, timeout=10, check=True)
            if any(flag not in help_result.stdout for flag in flags):
                raise qualification.QualificationError("installed selected client does not support the reviewed bounded driver flags")
            run_selected_client(argv, directory)
    state = gateway.action("setup", "readiness.state", {})
    evidence = next((check for check in state.get("checks", []) if check.get("check") == "selected_clients"), {})
    if evidence.get("status") != "verified":
        raise qualification.QualificationError("gateway did not observe successful actual tool use from every selected client")


def mcp(gateway: Gateway):
    metadata = json.loads(Path(required("LABBY_QUALIFICATION_TOKEN_FILE") + ".artifact.json").read_text())
    if metadata.get("url") != required("LABBY_QUALIFICATION_MCP_URL") or metadata.get("revisionId") != required("LABBY_QUALIFICATION_REVISION_ID"):
        raise qualification.QualificationError("approved MCP metadata changed")
    name = "first-use-qualification"
    spec = {"name": name, "url": metadata["url"], "command": None, "args": [], "bearer_token_env": None,
            "proxy_resources": False, "proxy_prompts": False, "proxy_skills": False, "proxy_mcp_ui": False}
    params = {"spec": spec, "confirm": True}
    if metadata["authentication"] == "bearer":
        spec["bearer_token_env"] = "LABBY_GATEWAY_FIRST_USE_QUALIFICATION_TOKEN"
        params["bearer_token_value"] = required("LABBY_QUALIFICATION_MCP_TOKEN")
    gateway.action("gateway", "gateway.add", params)
    gateway.action("gateway", "gateway.test", {"name": name, "confirm": True})
    tools = gateway.action("setup", "mcp.verification.tools", {"name": name, "expected_url": metadata["url"]})
    tool = required("LABBY_QUALIFICATION_TOOL")
    if tool not in [entry.get("name") for entry in tools.get("tools", [])]:
        raise qualification.QualificationError("approved tool is not an exposed eligible verification tool")
    arguments = json.loads(os.environ.get("LABBY_QUALIFICATION_TOOL_ARGUMENTS", "{}"))
    if not isinstance(arguments, dict):
        raise qualification.QualificationError("approved tool arguments must be an object")
    reviewed = [entry for entry in tools.get("tools", []) if entry.get("name") == tool]
    fingerprint = reviewed[0].get("reviewFingerprint") if len(reviewed) == 1 else None
    if not isinstance(fingerprint, str) or not fingerprint:
        raise qualification.QualificationError("tool review returned no stable approval fingerprint")
    result = gateway.action("setup", "mcp.verification.call", {"name": name, "expected_url": metadata["url"], "expected_fingerprint": fingerprint, "tool": tool, "arguments": arguments, "approved": True})
    if result.get("verified") is not True or result.get("server") != name or result.get("tool") != tool:
        raise qualification.QualificationError("approved MCP tool did not earn real verification evidence")
    external_clients(gateway, tool, arguments)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=qualification.STAGES)
    args = parser.parse_args()
    try:
        if args.stage == "bootstrap":
            bootstrap()
        else:
            globals()[args.stage](Gateway())
        return 0
    except Exception as error:
        print(str(error) if isinstance(error, qualification.QualificationError) else type(error).__name__, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
