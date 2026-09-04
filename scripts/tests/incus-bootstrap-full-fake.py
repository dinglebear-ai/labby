#!/usr/bin/env python3
import copy, json, os, pathlib, subprocess, tempfile, unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
CHECKPOINTS = ["storage","profile","container-launch","container-start","backup-config","hostname","binary","provision","readiness","tailscale-key","tailscale-up","tailscale-cleanup"]

class BootstrapFakeTest(unittest.TestCase):
    def baseline(self, customized):
        return {"name":"fixture","storage":customized,"storage_driver":"dir","storage_name":"pool","profiles":({"labby-gateway":"custom-profile\n"} if customized else {}),"container":customized,"running":False,"container_profiles":(["labby-gateway"] if customized else []),"config":{},"hostname":"custom-host" if customized else "", "netplan":"custom-netplan" if customized else "", "services":{"systemd-networkd":{"active":"active","enabled":"enabled"},"systemd-resolved":{"active":"active","enabled":"enabled"},"labby.service":{"active":"active","enabled":"enabled"}},"labby_failed_on_start":False,"binary":"prior-binary" if customized else None,"upload":None,"web":"prior-web" if customized else None,"web_backup":None,"owned":"prior-owned" if customized else None,"owned_backup":None,"tailscale":False,"ts_key":False}

    def run_bootstrap(self, work, state, fail_after=None, fail_list_column=None):
        state_path = work / "state.json"; state_path.write_text(json.dumps(state, sort_keys=True))
        binary = work / "candidate"; binary.write_text("candidate-binary")
        bin_dir = work / "bin"; bin_dir.mkdir(exist_ok=True)
        incus = bin_dir / "incus"
        if not incus.exists(): incus.symlink_to(ROOT / "scripts/tests/fake-incus.py")
        timeout = bin_dir / "timeout"; timeout.write_text("#!/bin/sh\nshift\nexec \"$@\"\n"); timeout.chmod(0o755)
        env = os.environ | {"PATH":f"{bin_dir}:/usr/bin:/bin","FAKE_INCUS_STATE":str(state_path)}
        if fail_after: env["LABBY_INCUS_FAIL_AFTER"] = fail_after
        if fail_list_column: env["FAKE_INCUS_FAIL_LIST_COLUMN"] = fail_list_column
        if fail_after and fail_after.startswith("tailscale-"): env["TS_AUTHKEY"] = "fixture-key"
        result = subprocess.run([str(ROOT / "scripts/incus-bootstrap.sh"),"--name","fixture","--version","v1.0.0","--local-binary",str(binary),"--storage-driver","dir","--storage-pool","pool","--no-backup-config"],cwd=ROOT,env=env,text=True,capture_output=True)
        return result, json.loads(state_path.read_text())

    def test_inventory_probe_failure_stops_before_launch(self):
        with tempfile.TemporaryDirectory() as td:
            before = self.baseline(False)
            result, after = self.run_bootstrap(
                pathlib.Path(td), copy.deepcopy(before), fail_list_column="n"
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("failed to query Incus container inventory", result.stderr)
            self.assertEqual(after, before)

    def test_status_probe_failure_stops_before_start(self):
        with tempfile.TemporaryDirectory() as td:
            before = self.baseline(True)
            result, after = self.run_bootstrap(
                pathlib.Path(td), copy.deepcopy(before), fail_list_column="s"
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("failed to query Incus container status", result.stderr)
            self.assertEqual(after, before)

    def test_fault_matrix_restores_new_and_customized_targets_and_reruns(self):
        for customized in (False, True):
            for checkpoint in CHECKPOINTS:
                if customized and checkpoint == "container-launch": continue
                if not customized and checkpoint == "container-start": continue
                with self.subTest(customized=customized, checkpoint=checkpoint), tempfile.TemporaryDirectory() as td:
                    before = self.baseline(customized)
                    result, after = self.run_bootstrap(pathlib.Path(td), copy.deepcopy(before), checkpoint)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(after, before, result.stdout + result.stderr)
            with tempfile.TemporaryDirectory() as td:
                work = pathlib.Path(td); before = self.baseline(customized)
                first, state = self.run_bootstrap(work, copy.deepcopy(before))
                self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
                second, state2 = self.run_bootstrap(work, state)
                self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
                self.assertEqual(state2, state)

    def test_fault_rollback_restores_exact_labby_unit_file_state(self):
        for unit_file_state in ("disabled", "enabled", "enabled-runtime"):
            with self.subTest(unit_file_state=unit_file_state), tempfile.TemporaryDirectory() as td:
                before = self.baseline(True)
                before["services"]["labby.service"]["enabled"] = unit_file_state
                result, after = self.run_bootstrap(pathlib.Path(td), copy.deepcopy(before), "readiness")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(after, before, result.stdout + result.stderr)

    def test_failed_enabled_runtime_service_and_executable_are_restored_exactly(self):
        with tempfile.TemporaryDirectory() as td:
            before = self.baseline(True)
            before["services"]["labby.service"] = {
                "active": "failed",
                "enabled": "enabled-runtime",
            }
            before["labby_failed_on_start"] = True
            before["binary"] = "known-failed-prior-binary"
            result, after = self.run_bootstrap(
                pathlib.Path(td), copy.deepcopy(before), "readiness"
            )
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("injected failure after readiness", result.stderr)
            self.assertEqual(after, before, result.stdout + result.stderr)
            self.assertEqual(after["binary"], "known-failed-prior-binary")

if __name__ == "__main__": unittest.main()
