"""Behavioral qualification of required release provenance sidecars."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BundleObservationTests(unittest.TestCase):
    def test_required_bundle_never_falls_back_to_account_api_and_extra_assets_stay_visible(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets = root / "assets"
            assets.mkdir()
            for name in ("archive.tar.gz", "archive.spdx.json"):
                (assets / name).write_text("fixture")
            manifest = assets / "release-manifest.json"
            manifest.write_text(json.dumps({
                "repository": "example/labby", "tag": "v1.0.0",
                "subjects": [{"name": "archive.tar.gz", "sbom": {"name": "archive.spdx.json"}}],
                "attestations": [{"subject": "archive.tar.gz"}],
                "provenance_bundles": [{"subject": "archive.tar.gz", "name": "archive.tar.gz.sigstore.jsonl"}],
                "distributions": {"github": {}, "npm": {"package": "fixture", "version": "1.0.0"}, "mcp": {"name": "fixture", "version": "1.0.0", "manifest_sha256": "fixture"}},
            }))
            binaries = root / "bin"
            binaries.mkdir()
            gh = binaries / "gh"
            gh.write_text("#!/bin/sh\ncase \"$1\" in\n --version) echo 'gh version 2.102.0'; exit 0;;\n attestation) [ \"$*\" != 'attestation verify --help' ] || exit 0; printf '%s\\n' \"$*\" >>\"$FIXTURE_CALLS\"; case \"$*\" in *'--bundle '*) exit 0;; *) exit 1;; esac;;\n release) printf false;;\n *) exit 1;;\nesac\n")
            gh.chmod(0o755)
            calls = root / "calls"
            environment = {**os.environ, "PATH": str(binaries) + os.pathsep + os.environ["PATH"], "GH_BIN": str(gh), "NPM_BIN": "/usr/bin/false", "CURL_BIN": "/usr/bin/false", "FIXTURE_CALLS": str(calls)}
            output = root / "observed.json"
            command = ["python3", str(ROOT / "scripts/ci/observe-release.py"), "--manifest", str(manifest), "--assets", str(assets), "--output", str(output)]
            subprocess.run(command, env=environment, check=True)
            self.assertEqual(json.loads(output.read_text())["attestations"][0]["status"], "missing_bundle")
            self.assertFalse(calls.exists())
            (assets / "archive.tar.gz.sigstore.jsonl").write_text("signed fixture")
            (assets / "unexpected.sigstore.jsonl").write_text("unrelated")
            subprocess.run(command, env=environment, check=True)
            observed = json.loads(output.read_text())
            self.assertEqual(observed["attestations"][0]["status"], "verified")
            self.assertEqual(observed["unexpected_assets"], ["unexpected.sigstore.jsonl"])
            self.assertIn("--bundle", calls.read_text())
