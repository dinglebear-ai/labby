"""Pinned verifier policy, generated trust inventory, and CI routing contracts."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class VerifierBootstrapTests(unittest.TestCase):
    def test_existing_verifier_requires_fixed_stable_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "gh"
            for version, accepted in (("2.101.0", False), ("2.102.0", True), ("2.102.1", True), ("2.102.0-pre", False), ("3.0.0", True), ("invalid", False)):
                executable.write_text(f"#!/bin/sh\nprintf '%s\\n' 'gh version {version}'\n")
                executable.chmod(0o700)
                result = subprocess.run(["sh", "-c", '. "$1"; github_verifier_version_ok "$2"', "test", str(ROOT / "scripts/ci/github-verifier-bootstrap.sh"), str(executable)], check=False)
                self.assertEqual(result.returncode == 0, accepted, version)

    def test_generated_helper_and_embedded_installer_are_reproducible(self):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            for relative in ("scripts/ci/github-verifier-bootstrap-pins.json", "scripts/ci/github-verifier-bootstrap.sh.in", "scripts/ci/github-verifier-bootstrap.sh", "scripts/ci/generate-verifier-bootstrap.py", "scripts/install.sh", "scripts/install.ps1", "scripts/ci/github-verifier-bootstrap.ps1.in"):
                destination = work / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, destination)
            expected = {p: (work / p).read_bytes() for p in ("scripts/ci/github-verifier-bootstrap.sh", "scripts/install.sh", "scripts/install.ps1")}
            subprocess.run(["python3", str(work / "scripts/ci/generate-verifier-bootstrap.py")], check=True)
            for path, contents in expected.items():
                self.assertEqual((work / path).read_bytes(), contents, path)
            pins = work / "scripts/ci/github-verifier-bootstrap-pins.json"
            data = json.loads(pins.read_text())
            data["assets"][0]["sha256"] = "invalid"
            pins.write_text(json.dumps(data))
            rejected = subprocess.run(["python3", str(work / "scripts/ci/generate-verifier-bootstrap.py")], capture_output=True, check=False)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertEqual((work / "scripts/ci/github-verifier-bootstrap.sh").read_bytes(), expected["scripts/ci/github-verifier-bootstrap.sh"])

    def test_trust_inputs_route_to_release_contract(self):
        spec = importlib.util.spec_from_file_location("changed_paths", ROOT / "scripts/ci/changed_paths.py")
        classifier = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(classifier)
        for name in ("github-verifier-bootstrap.sh", "github-verifier-bootstrap.sh.in", "github-verifier-bootstrap.ps1.in", "github-verifier-bootstrap-pins.json", "generate-verifier-bootstrap.py", "verify-release-provenance.sh", "export-release-attestation-bundles.sh", "test_verifier_bootstrap.py", "test_release_provenance_bundles.py"):
            with self.subTest(name=name):
                flags = classifier.classify("pull_request", ["scripts/ci/" + name])
                self.assertTrue(flags["workflow"] or flags["release"])
                self.assertFalse(flags["rust_compile"])

    def test_bundle_verification_clears_credentials_and_isolates_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            gh = work / "gh"
            log = work / "environment.json"
            gh.write_text("#!/usr/bin/env python3\nimport json, os, sys\nif sys.argv[1:] == ['--version']: print('gh version 2.102.0')\nelif sys.argv[1:] == ['attestation', 'verify', '--help']: pass\nelse:\n assert '--bundle' in sys.argv\n assert all(not os.environ.get(k) for k in ('GH_TOKEN','GITHUB_TOKEN','GH_ENTERPRISE_TOKEN','GITHUB_ENTERPRISE_TOKEN'))\n assert os.environ['GH_HOST'] == 'github.com'\n assert os.listdir(os.environ['GH_CONFIG_DIR']) == []\n with open(os.environ['FIXTURE_ENV_LOG'],'w') as f: json.dump({'config':os.environ['GH_CONFIG_DIR']},f)\n")
            gh.chmod(0o700)
            artifact, bundle = work / "artifact", work / "bundle"
            artifact.write_text("fixture")
            bundle.write_text("fixture")
            environment = os.environ | {"GH_VERIFIER": str(gh), "GH_TOKEN": "fixture-private", "GITHUB_TOKEN": "fixture-private", "GH_ENTERPRISE_TOKEN": "fixture-private", "GITHUB_ENTERPRISE_TOKEN": "fixture-private", "GH_HOST": "enterprise.invalid", "GH_CONFIG_DIR": str(work / "existing"), "FIXTURE_ENV_LOG": str(log)}
            subprocess.run(["bash", str(ROOT / "scripts/ci/verify-release-provenance.sh"), "--repo", "example/labby", "--workflow", "release.yml", "--ref", "refs/tags/v1", "--artifact", str(artifact), "--bundle", str(bundle)], env=environment, check=True)
            self.assertFalse(Path(json.loads(log.read_text())["config"]).exists(), "temporary config must be cleaned")
