#!/usr/bin/env python3
"""Driver operation contract fixtures; not end-to-end release evidence."""
import importlib.util
import json
import os
import sys
import time
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("driver", Path(__file__).with_name("first-use-native-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class FakeGateway:
    def __init__(self):
        self.calls = []
        self.artifact = {"providerId": "public", "artifactId": "approved", "currentRevisionId": "revision",
                         "mcpConnection": {"schemaVersion": "labby.mcp-connection/v1", "revisionId": "revision", "transport": "http", "authentication": "none", "url": "https://mcp.example.com/mcp"}}
    def principal(self):
        return "authenticated-owner"
    def request(self, path, body):
        self.calls.append((path, body))
        return {"state": "complete", "coverageComplete": True, "failures": [], "items": [self.artifact]}
    def action(self, service, action, params):
        self.calls.append((action, params))
        if action == "readiness.state":
            return {"checks": [{"check": "selected_clients", "status": "verified" if getattr(self, "clients_verified", False) else "pending"}]}
        if action == "agents.models.list":
            return {"models": ["verified-model"]}
        if action == "agents.run":
            return {"status": "completed", "output": "Real-provider output fixture"}
        if action.endswith("verification.tools"):
            return {"tools": [{"name": "version", "reviewFingerprint": "review"}]}
        if action.endswith("verification.call"):
            return {"verified": True, "server": "first-use-qualification", "tool": "version"}
        return {}


class DriverTests(unittest.TestCase):
    def test_client_timeout_stops_delayed_child(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'child-ran'
            started = Path(directory) / 'child-started'
            child = 'import time; from pathlib import Path; Path(' + repr(str(started)) + ').write_text("started"); time.sleep(1); Path(' + repr(str(marker)) + ').write_text("unexpected")'
            parent = 'import subprocess, sys, time; subprocess.Popen([sys.executable, "-c", ' + repr(child) + ']); time.sleep(30)'
            with self.assertRaises(driver.qualification.QualificationError):
                driver.run_selected_client([sys.executable, '-c', parent], directory, timeout=0.5)
            self.assertTrue(started.exists(), 'the descendant fixture never started')
            time.sleep(1.1)
            self.assertFalse(marker.exists(), 'a descendant survived the client timeout')

    def test_outer_qualification_timeout_stops_client_group(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / 'child-ran'
            started = Path(directory) / 'child-started'
            child = 'import time; from pathlib import Path; Path(' + repr(str(started)) + ').write_text("started"); time.sleep(1); Path(' + repr(str(marker)) + ').write_text("unexpected")'
            parent = 'import subprocess, sys, time; subprocess.Popen([sys.executable, "-c", ' + repr(child) + ']); time.sleep(30)'
            stage = ('import importlib.util; spec=importlib.util.spec_from_file_location("driver", ' + repr(driver.__file__) + '); '
                     'module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module); '
                     'module.run_selected_client(' + repr([sys.executable, '-c', parent]) + ', ' + repr(directory) + ', timeout=30)')
            with self.assertRaises(driver.qualification.QualificationError):
                driver.qualification.command([sys.executable, '-c', stage], os.environ.copy(), time.monotonic() + 0.5)
            self.assertTrue(started.exists(), 'the descendant fixture never started')
            time.sleep(1.1)
            self.assertFalse(marker.exists(), 'a client descendant survived the outer deadline')

    def test_agent_uses_settings_and_current_identity_and_real_run(self):
        gateway = FakeGateway()
        with patch.dict(os.environ, {"LABBY_QUALIFICATION_PROVIDER_URL": "https://provider.example.com/v1", "LABBY_QUALIFICATION_PROVIDER_KEY": "protected", "LABBY_QUALIFICATION_MODEL": "verified-model"}):
            driver.agent(gateway)
        self.assertEqual([call[0] for call in gateway.calls], ["settings.env.update", "agents.models.list", "agents.create", "agents.run"])
        self.assertEqual(gateway.calls[2][1]["owner_id"], "authenticated-owner")
        self.assertIn({"key": "LABBY_AGENT_PROVIDER_PROTOCOL", "value": "openai"}, gateway.calls[0][1]["entries"])

    def test_wrong_provider_model_cannot_create_agent(self):
        gateway = FakeGateway()
        with patch.dict(os.environ, {"LABBY_QUALIFICATION_PROVIDER_URL": "https://provider.example.com/v1", "LABBY_QUALIFICATION_MODEL": "invented"}):
            with self.assertRaises(driver.qualification.QualificationError):
                driver.agent(gateway)
        self.assertNotIn("agents.create", [call[0] for call in gateway.calls])

    def test_discovery_mcp_requires_reviewed_revision_and_safe_tool_operation(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"LABBY_QUALIFICATION_ARTIFACT_ID": "approved", "LABBY_QUALIFICATION_REVISION_ID": "revision", "LABBY_QUALIFICATION_MCP_URL": "https://mcp.example.com/mcp", "LABBY_QUALIFICATION_QUERY": "approved", "LABBY_QUALIFICATION_TOKEN_FILE": str(Path(directory) / "token"), "LABBY_QUALIFICATION_TOOL": "version", "LABBY_QUALIFICATION_SELECTED_CLIENTS": "[]"}
            gateway = FakeGateway()
            with patch.dict(os.environ, env):
                driver.discover(gateway)
                driver.mcp(gateway)
            self.assertEqual([call[0] for call in gateway.calls], ["/v1/depot/discover", "gateway.add", "gateway.test", "mcp.verification.tools", "mcp.verification.call"])
            self.assertFalse(gateway.calls[1][1]["spec"]["proxy_resources"])
            self.assertTrue(gateway.calls[-1][1]["approved"])
            self.assertEqual(gateway.calls[-1][1]["expected_fingerprint"], "review")
            gateway.artifact["currentRevisionId"] = "changed"
            with patch.dict(os.environ, env):
                with self.assertRaises(driver.qualification.QualificationError):
                    driver.discover(gateway)

    def test_actual_selected_cli_commands_are_bounded_and_need_gateway_observation(self):
        gateway = FakeGateway()
        environment = {"LABBY_QUALIFICATION_SELECTED_CLIENTS": '["codex","claude-code"]', "LABBY_QUALIFICATION_CODEX_MODEL": "chosen-codex", "LABBY_QUALIFICATION_CLAUDE_MODEL": "chosen-claude"}
        help_text = "--sandbox --strict-config --ephemeral --skip-git-repo-check --model --config --print --tools --allowedTools --permission-mode --no-session-persistence --max-budget-usd"
        exposed = [{"name": "version", "annotations": {"readOnlyHint": True, "destructiveHint": False}}]
        with patch.dict(os.environ, environment), patch.object(driver, "advertised_tools", return_value=exposed), patch.object(driver.subprocess, "run", side_effect=lambda argv, **kwargs: SimpleNamespace(stdout=json.dumps([{"name": "lab"}, {"name": "other-server"}]) if argv[:3] == ["codex", "mcp", "list"] else help_text)), patch.object(driver, 'run_selected_client') as process:
            with self.assertRaises(driver.qualification.QualificationError):
                driver.external_clients(gateway, "version", {})
            commands = [call.args[0] for call in process.call_args_list]
            self.assertEqual(commands[0][0], "codex")
            self.assertIn("read-only", commands[0])
            self.assertIn('mcp_servers."other-server".enabled=false', commands[0])
            self.assertIn('mcp_servers.lab.enabled_tools=["version"]', commands[0])
            self.assertEqual(commands[1][0], "claude")
            self.assertIn("mcp__lab__version", commands[1])
            self.assertIn("dontAsk", commands[1])
            self.assertTrue(all(not any("dangerously" in value for value in command) for command in commands))
            gateway.clients_verified = True
            driver.external_clients(gateway, "version", {})

    def test_client_registration_is_not_a_success_receipt(self):
        gateway = FakeGateway()
        with patch.dict(os.environ, {"LABBY_QUALIFICATION_SELECTED_CLIENTS": '["codex"]', "LABBY_QUALIFICATION_BINARY": "/approved/release/labby"}), patch.object(driver.subprocess, "run") as process:
            driver.clients(gateway)
            self.assertEqual(process.call_args.args[0], ["/approved/release/labby", "setup", "clients", "connect", "--clients", "codex"])
        self.assertEqual(gateway.calls, [])

    def test_no_client_selection_deferred_selected_clients_never_faked(self):
        gateway = FakeGateway()
        with patch.dict(os.environ, {"LABBY_QUALIFICATION_SELECTED_CLIENTS": "[]"}):
            driver.clients(gateway)
        self.assertEqual(gateway.calls[0][0], "readiness.clients.defer")
        with patch.dict(os.environ, {"LABBY_QUALIFICATION_SELECTED_CLIENTS": '["codex"]'}):
            with self.assertRaises(driver.qualification.QualificationError):
                driver.clients(gateway)
        self.assertEqual(len(gateway.calls), 1)


if __name__ == "__main__":
    unittest.main()
