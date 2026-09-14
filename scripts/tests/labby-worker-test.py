"""Regression tests for rejection of the unsafe systemd user-manager profile."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
PLUGIN = ROOT / "unraid/source/usr/local/emhttp/plugins/labby"
INIT = PLUGIN / "scripts/labby-incus-init.sh"


class WorkerProfileTests(unittest.TestCase):
    def probe(self, enabled, residual=False, query_failed=False, loaded=False, live=False, resources=False, action=None):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory)
            (prefix / "bin").mkdir()
            (prefix / "labby.cfg").write_text("RUNTIME_MODE=incus\n")
            incus = prefix / "bin/incus"
            incus.write_text("#!/bin/sh\nexit 0\n")
            incus.chmod(0o755)
            for command in ("curl", "flock", "ip", "sha256sum", "timeout"):
                (prefix / "bin" / command).symlink_to(incus)
            for name, content in {"systemctl": "#!/bin/sh\nexit 0\n"}.items():
                command_path = prefix / "bin" / name
                command_path.write_text(content)
                command_path.chmod(0o755)
            script = r'''
source "$1"
LABBY_SERVICE_WORKERS_ENABLED=$2
incus_exec() {
    if [ "$1" = 30 ]; then shift; "$@"; return; fi
    # Resource convergence executes against the temporary drop-in directory.
    # Worker guards may only inspect the guest; unexpected operations fail.
    if [ "$#" -eq 4 ] && [ "$1" = 10 ] && [ "$2" = sh ] && [ "$3" = -c ] &&
       [ "$4" = "[ ! -e /etc/systemd/system/labby.service.d/workers.conf ]" ]; then
        return "$RESIDUAL_EXIT"
    fi
    if [ "$*" = "10 systemctl show labby.service --property=MainPID --property=BindPaths" ]; then
        if [ "$LOADED" = true ]; then
            printf '%s\n' 'MainPID=123' 'BindPaths=/run/user/1000'
        elif [ "$LIVE" = true ]; then
            printf '%s\n' 'MainPID=123' 'BindPaths='
        else
            printf '%s\n' 'MainPID=0' 'BindPaths='
        fi
        return 0
    fi
    if [ "$#" -eq 6 ] && [ "$1" = 10 ] && [ "$2" = sh ] && [ "$3" = -c ] &&
       [ "$5" = sh ] && [ "$6" = 123 ]; then
        [ "$LIVE" != true ]
        return
    fi
    echo "unexpected container operation: $*" >&2
    return 77
}
"$3"
'''
            result = subprocess.run(
                ["bash", "-c", script, "test", str(INIT), enabled,
                 action or ("ensure_service_resource_limits" if resources else "ensure_service_worker_profile")],
                env={**os.environ, "EMHTTP": str(PLUGIN),
                     "INCUS_PREFIX": str(prefix), "LABBY_SERVICE_DROPIN_DIR": str(prefix / "dropin"), "INCUS_CONFIG": str(prefix / "absent"),
                     "CFG": str(prefix / "labby.cfg"), "LABBY_INCUS_INIT_LIBRARY": "1",
                     "RESIDUAL_EXIT": "73" if query_failed else "1" if residual else "0",
                     "LOADED": "true" if loaded else "false", "LIVE": "true" if live else "false"},
                capture_output=True, text=True)
            result.dropin = (prefix / "dropin/resource-limits.conf").read_text() if resources else ""
            return result

    def test_resource_policy_masks_stable_user_runtime_parent(self):
        result = self.probe("false", resources=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("InaccessiblePaths=/run/user", result.dropin)

    def test_preflight_allows_live_socket_until_hardened_restart(self):
        result = self.probe("false", live=True, action="ensure_service_worker_profile_config")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_runtime_guard_rejects_live_socket(self):
        result = self.probe("false", live=True, action="ensure_service_worker_runtime")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restart", result.stdout + result.stderr)

    def test_enabling_profile_is_rejected_without_installing_it(self):
        result = self.probe("true")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("user-manager socket", result.stdout + result.stderr)
        self.assertNotIn("unexpected container operation", result.stderr)

    def test_disabled_profile_rejects_residual_dropin(self):
        result = self.probe("false", residual=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restore the original upstream commands", result.stdout + result.stderr)

    def test_disabled_profile_accepts_absent_dropin(self):
        result = self.probe("false")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_failed_container_query_does_not_look_like_safe_absence(self):
        result = self.probe("false", query_failed=True)
        self.assertNotEqual(result.returncode, 0)

    def test_loaded_manager_bind_is_rejected_after_dropin_deleted(self):
        result = self.probe("false", loaded=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reload systemd and restart", result.stdout + result.stderr)

    def test_running_manager_socket_is_rejected_after_daemon_reload(self):
        result = self.probe("false", live=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restart", result.stdout + result.stderr)

    def test_invalid_flag_is_rejected(self):
        result = self.probe("perhaps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be true or false", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
