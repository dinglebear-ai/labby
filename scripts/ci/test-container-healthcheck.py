#!/usr/bin/env python3
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = pathlib.Path(__file__).parents[2]


class ContainerHealthcheckTest(unittest.TestCase):
    def failing_environment(self, directory, **overrides):
        state = pathlib.Path(directory) / "state"
        fake_bin = pathlib.Path(directory) / "bin"
        fake_bin.mkdir()
        curl = fake_bin / "curl"
        curl.write_text("#!/bin/sh\nexit 1\n")
        curl.chmod(0o755)
        return state, {
            **os.environ,
            "PATH": f"{fake_bin}:{os.environ['PATH']}",
            "LABBY_HEALTH_STATE_DIR": str(state),
            "LABBY_HEALTH_TEST_MODE": "1",
            **overrides,
        }

    def test_failure_recovery_is_persistent_bounded_and_redacted(self):
        with tempfile.TemporaryDirectory() as directory:
            state = pathlib.Path(directory) / "state"
            fake_bin = pathlib.Path(directory) / "bin"
            fake_bin.mkdir()
            curl = fake_bin / "curl"
            curl.write_text("#!/bin/sh\nexit 1\n")
            curl.chmod(0o755)
            env = {
                **os.environ,
                "PATH": f"{fake_bin}:{os.environ['PATH']}",
                "LABBY_HEALTH_STATE_DIR": str(state),
                "LABBY_HEALTH_TEST_MODE": "1",
                "LABBY_HEALTH_LOG_MAX_BYTES": "4096",
                "LABBY_HEALTH_LOG_KEEP_BYTES": "2048",
            }
            for _ in range(9):
                result = subprocess.run(
                    [ROOT / "scripts/ci/container-healthcheck.sh"],
                    env=env,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 1)
            self.assertEqual((state / "health-failures").read_text().strip(), "9")
            log = (state / "health-recovery.log").read_text()
            self.assertEqual(log.count("restart_requested"), 6)
            delays = [int(line.rsplit("=", 1)[1]) for line in log.splitlines() if "restart_requested" in line]
            self.assertLess(max(delays), 5, "recovery delay exceeds the Compose healthcheck timeout")
            self.assertIn("recovery exhausted", result.stderr)
            self.assertNotIn("Authorization", log)
            env["LABBY_HEALTH_LOG_MAX_BYTES"] = "256"
            env["LABBY_HEALTH_LOG_KEEP_BYTES"] = "128"
            for _ in range(20):
                subprocess.run(
                    [ROOT / "scripts/ci/container-healthcheck.sh"],
                    env=env,
                    check=False,
                    capture_output=True,
                    text=True,
                )
            self.assertLessEqual((state / "health-recovery.log").stat().st_size, 256)

    def test_keep_limit_is_clamped_to_maximum(self):
        with tempfile.TemporaryDirectory() as directory:
            state, env = self.failing_environment(
                directory,
                LABBY_HEALTH_LOG_MAX_BYTES="10",
                LABBY_HEALTH_LOG_KEEP_BYTES="100",
            )
            result = subprocess.run(
                [ROOT / "scripts/ci/container-healthcheck.sh"], env=env, check=False
            )
            self.assertEqual(result.returncode, 1)
            self.assertLessEqual((state / "health-recovery.log").stat().st_size, 10)

    def test_invalid_log_limits_fail_closed_without_writing_state(self):
        for key, value in [
            ("LABBY_HEALTH_LOG_MAX_BYTES", "0"),
            ("LABBY_HEALTH_LOG_KEEP_BYTES", "0"),
            ("LABBY_HEALTH_LOG_MAX_BYTES", "invalid"),
            ("LABBY_HEALTH_LOG_KEEP_BYTES", "1.5"),
        ]:
            with self.subTest(key=key, value=value), tempfile.TemporaryDirectory() as directory:
                state, env = self.failing_environment(directory, **{key: value})
                result = subprocess.run(
                    [ROOT / "scripts/ci/container-healthcheck.sh"],
                    env=env,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 64)
                self.assertIn("positive integer", result.stderr)
                self.assertFalse((state / "health-recovery.log").exists())

    def test_hung_health_endpoint_is_bounded_and_enters_persistent_recovery(self):
        with tempfile.TemporaryDirectory() as directory:
            state = pathlib.Path(directory) / "state"
            state.mkdir()
            (state / "health-failures").write_text("2\n")
            ready = threading.Event()

            def hang_after_accept():
                with socket.socket() as server:
                    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                    server.bind(("127.0.0.1", 8765))
                    server.listen(1)
                    ready.set()
                    connection, _ = server.accept()
                    with connection:
                        time.sleep(10)

            thread = threading.Thread(target=hang_after_accept, daemon=True)
            thread.start()
            self.assertTrue(ready.wait(1), "hanging health fixture did not start")
            started = time.monotonic()
            result = subprocess.run(
                [ROOT / "scripts/ci/container-healthcheck.sh"],
                env={
                    **os.environ,
                    "LABBY_HEALTH_STATE_DIR": str(state),
                    "LABBY_HEALTH_TEST_MODE": "1",
                },
                check=False,
                capture_output=True,
                text=True,
                timeout=4,
            )

            self.assertEqual(result.returncode, 1)
            self.assertLess(time.monotonic() - started, 4)
            self.assertEqual((state / "health-failures").read_text().strip(), "3")
            self.assertIn("restart_requested delay=1", (state / "health-recovery.log").read_text())


if __name__ == "__main__":
    unittest.main()
