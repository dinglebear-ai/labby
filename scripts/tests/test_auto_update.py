import importlib.util
from pathlib import Path
import plistlib
import unittest
from unittest.mock import patch
import tempfile
import io
import json
import subprocess

spec = importlib.util.spec_from_file_location(
    "auto_update", Path(__file__).parents[1] / "auto-update.py"
)
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


def release(tag, **kwargs):
    return dict(
        tag_name=tag,
        assets=[{"name": updater.ASSET}, {"name": updater.ASSET + ".sha256"}],
        **kwargs
    )


class AutoUpdateTests(unittest.TestCase):
    def test_skips_drafts_prereleases_and_nonbinary_releases(self):
        releases = [
            release("v9.0.0", draft=True),
            release("v8.0.0", prerelease=True),
            release("v2.0.0-rc.1"),
            dict(tag_name="v7.0.0", assets=[]),
            release("v1.17.0"),
        ]
        self.assertEqual(updater.select_release(releases, (1, 16, 1)), "v1.17.0")

    def test_never_downgrades_or_reinstalls(self):
        self.assertIsNone(
            updater.select_release([release("v1.13.3"), release("v1.16.1")], (1, 16, 1))
        )

    def test_selects_highest_version_not_api_order(self):
        self.assertEqual(
            updater.select_release([release("v1.9.0"), release("v1.10.0")], (1, 8, 0)),
            "v1.10.0",
        )

    def test_unknown_installed_version_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(
                updater.subprocess, "check_output", return_value="labby dev"
            ), patch.object(updater.urllib.request, "urlopen") as fetch:
                with self.assertRaises(ValueError):
                    updater.run(
                        Path(directory) / "labby", Path(directory) / "install.sh"
                    )
                fetch.assert_not_called()

    def test_verified_installer_is_pinned_and_overrides_are_removed(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "labby"
            response = io.BytesIO(json.dumps([release("v1.17.0")]).encode())
            with patch.object(
                updater.subprocess,
                "check_output",
                side_effect=["labby 1.16.1", "labby 1.17.0"],
            ), patch.object(
                updater.urllib.request, "urlopen", return_value=response
            ), patch.object(
                updater.subprocess, "run"
            ) as install, patch.dict(
                updater.os.environ,
                {
                    "LABBY_INSTALL_LOCAL_BINARY": "/untrusted",
                    "LABBY_INSTALL_ROLLBACK": "1",
                    "LABBY_ALLOW_SOURCE_FALLBACK": "1",
                },
            ):
                updater.run(binary, Path("/trusted/install.sh"))
                args, kwargs = install.call_args
                self.assertEqual(args[0], ["sh", "/trusted/install.sh"])
                self.assertEqual(kwargs["env"]["LABBY_INSTALL_VERSION"], "v1.17.0")
                self.assertEqual(kwargs["env"]["LABBY_INSTALL_DIR"], directory)
                self.assertEqual(kwargs["env"]["LABBY_ALLOW_SOURCE_FALLBACK"], "0")
                self.assertNotIn("LABBY_INSTALL_LOCAL_BINARY", kwargs["env"])
                self.assertNotIn("LABBY_INSTALL_ROLLBACK", kwargs["env"])
                self.assertTrue(kwargs["check"])

    def test_verification_failure_is_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            response = io.BytesIO(json.dumps([release("v1.17.0")]).encode())
            with patch.object(
                updater.subprocess, "check_output", return_value="labby 1.16.1"
            ) as read, patch.object(
                updater.urllib.request, "urlopen", return_value=response
            ), patch.object(
                updater.subprocess,
                "run",
                side_effect=subprocess.CalledProcessError(1, "sh"),
            ):
                with self.assertRaises(subprocess.CalledProcessError):
                    updater.run(Path(directory) / "labby", Path("/installer"))
                self.assertEqual(read.call_count, 1)

    def test_plist_preserves_paths_as_arguments(self):
        result = plistlib.loads(
            plistlib.dumps(
                updater.job(
                    "/python",
                    "/Application Support/runner.py",
                    "/custom bin/labby",
                    "/installer",
                    "/log",
                    "/bin",
                )
            )
        )
        self.assertEqual(
            result["ProgramArguments"][1], "/Application Support/runner.py"
        )
        self.assertEqual(result["ProgramArguments"][4], "/custom bin/labby")
        self.assertEqual(result["StartInterval"], 86400)
        self.assertTrue(result["RunAtLoad"])


if __name__ == "__main__":
    unittest.main()
