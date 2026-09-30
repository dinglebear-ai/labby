"""Receipt checks, including real loopback HTTP requests."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "verify_handoff.py"
spec = importlib.util.spec_from_file_location("verify_handoff", SCRIPT)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.artifact = Path(self.tmp.name) / "release.tar"
        self.artifact.write_bytes(b"workflow fixture")
        self.sha = hashlib.sha256(self.artifact.read_bytes()).hexdigest()
        self.m = {
            "schema_version": 1, "environment": "staging",
            "source": {"repository": "example/project", "branch": "task/workflow", "commit": "a"*40, "published_commit": "a"*40, "dirty": False},
            "artifact": {"kind": "workflow-fixture", "sha256": self.sha},
            "sandbox": {"name": "workflow-stage", "development_name": "workflow-dev",
                "image": "docker.io/library/ubuntu@sha256:" + "b"*64,
                "cpus": 1, "memory_mib": 512, "workdir": "/workspace/release",
                "command": ["python3", "fixture_server.py"],
                "ports": [{"host_bind": "127.0.0.1", "host_port": 18767, "guest_port": 8080}],
                "persistent": True, "max_duration_secs": None, "idle_timeout_secs": None,
                "mounts": [], "env": {}, "secret_names": []},
            "checks": [{"name": "tests", "passed": True, "count": 1, "command": ["python3", "-m", "unittest"]},
                       {"name": "docs", "passed": True, "command": ["python3", "check-links.py"]}],
            "health_url": "http://127.0.0.1:18767/healthz"}
    def errors(self): return gate.validate(self.m, self.artifact)
    def reject(self, section, key, value):
        self.m[section][key] = value
        self.assertTrue(self.errors())
    def test_valid_receipt(self): self.assertEqual(self.errors(), [])
    def test_production(self): self.m["environment"] = "production"; self.assertTrue(self.errors())
    def test_same_sandbox(self): self.reject("sandbox", "name", "workflow-dev")
    def test_bad_name(self): self.reject("sandbox", "name", "../production")
    def test_mutable_image(self): self.reject("sandbox", "image", "ubuntu:latest")
    def test_zero_cpu(self): self.reject("sandbox", "cpus", 0)
    def test_boolean_cpu(self): self.reject("sandbox", "cpus", True)
    def test_missing_memory(self): del self.m["sandbox"]["memory_mib"]; self.assertTrue(self.errors())
    def test_relative_workdir(self): self.reject("sandbox", "workdir", "workspace")
    def test_parent_workdir(self): self.reject("sandbox", "workdir", "/workspace/../etc")
    def test_empty_command(self): self.reject("sandbox", "command", [])
    def test_ephemeral(self): self.reject("sandbox", "persistent", False)
    def test_expiry(self): self.reject("sandbox", "max_duration_secs", 86400)
    def test_idle_expiry(self): self.reject("sandbox", "idle_timeout_secs", 30)
    def test_missing_lifecycle(self): del self.m["sandbox"]["max_duration_secs"]; self.assertTrue(self.errors())
    def test_public_bind(self): self.m["sandbox"]["ports"][0]["host_bind"] = "0.0.0.0"; self.assertTrue(self.errors())
    def test_bad_port(self): self.m["sandbox"]["ports"][0]["host_port"] = 0; self.assertTrue(self.errors())
    def test_duplicate_port(self): self.m["sandbox"]["ports"] *= 2; self.assertTrue(self.errors())
    def test_unmapped_health(self): self.m["health_url"] = "http://127.0.0.1:9999/healthz"; self.assertTrue(self.errors())
    def test_remote_health(self): self.m["health_url"] = "http://example.com/healthz"; self.assertTrue(self.errors())
    def test_url_credentials(self): self.m["health_url"] = "http://user:secret@127.0.0.1:18767/healthz"; self.assertTrue(self.errors())
    def test_short_commit(self): self.reject("source", "commit", "1234567")
    def test_unpublished_commit(self): self.reject("source", "published_commit", "c"*40)
    def test_dirty_tree(self): self.reject("source", "dirty", True)
    def test_unknown_dirty(self): del self.m["source"]["dirty"]; self.assertTrue(self.errors())
    def test_corrupt_artifact(self): self.artifact.write_bytes(b"changed"); self.assertTrue(self.errors())
    def test_missing_artifact(self): self.artifact.unlink(); self.assertTrue(self.errors())
    def test_symlink_artifact(self):
        other=self.artifact.with_suffix(".real"); self.artifact.rename(other); self.artifact.symlink_to(other); self.assertTrue(self.errors())
    def test_failed_tests(self): self.m["checks"][0]["passed"] = False; self.assertTrue(self.errors())
    def test_zero_tests(self): self.m["checks"][0]["count"] = 0; self.assertTrue(self.errors())
    def test_missing_docs(self): self.m["checks"].pop(); self.assertTrue(self.errors())
    def test_raw_secret(self): self.reject("sandbox", "env", {"TOKEN": "secret"})
    def test_host_mount(self): self.reject("sandbox", "mounts", ["~/.ssh:/root/.ssh"])
    def test_unknown_schema(self): self.m["schema_version"] = 2; self.assertTrue(self.errors())
    def test_bad_nested_type(self): self.m["sandbox"] = []; self.assertTrue(self.errors())
    def test_bad_root_type(self): self.assertTrue(gate.validate([], self.artifact))
    def test_artifact_kind(self): self.reject("artifact", "kind", "application-maybe")
    def test_snapshot_not_release(self): self.m["snapshot"] = "checkpoint"; self.assertTrue(self.errors())
    def serve(self, reply, status=200, headers=None):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(status)
                for key, value in (headers or {}).items(): self.send_header(key,value)
                self.end_headers(); self.wfile.write(json.dumps(reply).encode())
            def log_message(self,*args): pass
        server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
        worker=threading.Thread(target=server.serve_forever,daemon=True); worker.start()
        self.addCleanup(worker.join,2); self.addCleanup(server.server_close); self.addCleanup(server.shutdown)
        self.m["sandbox"]["ports"][0]["host_port"]=server.server_port
        self.m["health_url"]=f"http://127.0.0.1:{server.server_port}/healthz"
    def test_real_http_identity(self):
        self.serve({"environment":"staging","sandbox":"workflow-stage","commit":"a"*40,"artifact_sha256":self.sha})
        self.assertEqual(gate.probe(self.m),[])
    def test_real_http_wrong_commit(self):
        self.serve({"environment":"staging","sandbox":"workflow-stage","commit":"c"*40,"artifact_sha256":self.sha})
        self.assertTrue(gate.probe(self.m))
    def test_real_http_wrong_sandbox(self):
        self.serve({"environment":"staging","sandbox":"wrong","commit":"a"*40,"artifact_sha256":self.sha})
        self.assertTrue(gate.probe(self.m))
    def test_real_http_unhealthy(self): self.serve({},503); self.assertTrue(gate.probe(self.m))
    def test_real_http_redirect(self): self.serve({},302,{"Location":"http://example.com/"}); self.assertTrue(gate.probe(self.m))
    def test_real_http_non_object(self): self.serve([]); self.assertTrue(gate.probe(self.m))

if __name__ == "__main__": unittest.main()
