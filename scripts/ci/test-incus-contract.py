#!/usr/bin/env python3
import os
import pathlib
import re
import stat
import sys
import unittest
import subprocess
import json
import shutil
import tempfile
import hashlib
import time

ROOT = pathlib.Path(os.environ.get("LABBY_TOPOLOGY_ROOT", pathlib.Path(__file__).parents[2]))


class IncusContract(unittest.TestCase):
    """Incus supply, image-definition, install-guidance, and rolling-pointer contracts."""

    def text(self, path):
        return (ROOT / path).read_text()

    def test_incus_sources_are_https(self):
        text = self.text("config/incus/labby-image.yaml")
        self.assertNotIn("url: http://", text)
        self.assertNotIn("mirror: http://", text)
        self.assertIn("https://snapshot.ubuntu.com/ubuntu/", text)
        self.assertNotIn('uv" python install', text)

    def incus_apt_repository(self):
        image = self.text("config/incus/labby-image.yaml")
        repositories = image.split("  repositories:\n", 1)[1].split("  sets:\n", 1)[0]
        names = re.findall(r"^    - name: (.+)$", repositories, re.MULTILINE)
        self.assertEqual(names, ["sources.list"])
        release = re.search(r"^  release: (\S+)$", image, re.MULTILINE).group(1)
        sources = [line.strip().replace("{{ image.release }}", release)
                   for line in repositories.splitlines() if line.strip().startswith("deb ")]
        return sources

    def test_incus_apt_repository_replaces_bootstrap_sources_with_all_pinned_suites(self):
        self.assertEqual(self.incus_apt_repository(), [
            "deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20260904T000000Z resolute main restricted universe multiverse",
            "deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20260904T000000Z resolute-updates main restricted universe multiverse",
            "deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/20260904T000000Z resolute-security main restricted universe multiverse",
        ])

    def test_apt_accepts_replacement_but_rejects_duplicate_snapshot_options(self):
        apt_get = shutil.which("apt-get")
        if apt_get is None:
            if sys.platform.startswith("linux"):
                self.fail("Linux Incus contract tests require apt-get for source validation")
            self.skipTest("apt-get is unavailable on this non-Linux host")
        sources = "\n".join(self.incus_apt_repository()) + "\n"
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            apt = root / "etc/apt"
            for path in [apt / "sources.list.d", apt / "apt.conf.d",
                         root / "state/lists/partial", root / "cache/archives/partial", root / "log"]:
                path.mkdir(parents=True)
            config = apt / "apt.conf"
            config.write_text("")
            status = root / "state/status"
            status.write_text("")
            command = [apt_get, "-o", f"Dir={root}", "-o", f"Dir::Etc={apt}",
                       "-o", "Dir::Etc::main=apt.conf", "-o", "Dir::Etc::parts=apt.conf.d",
                       "-o", "Dir::Etc::sourcelist=sources.list",
                       "-o", "Dir::Etc::sourceparts=sources.list.d",
                       "-o", f"Dir::State={root / 'state'}", "-o", f"Dir::State::status={status}",
                       "-o", f"Dir::Cache={root / 'cache'}", "-o", f"Dir::Log={root / 'log'}",
                       "indextargets"]
            env = dict(os.environ, APT_CONFIG=str(config), LC_ALL="C")
            # Reproduce debootstrap's base entry plus distrobuilder's former ubuntu.list.
            baseline = apt / "sources.list"
            baseline.write_text("deb https://snapshot.ubuntu.com/ubuntu/20260904T000000Z resolute main\n")
            duplicate = apt / "sources.list.d/ubuntu.list"
            duplicate.write_text(sources)
            broken = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(broken.returncode, 100, broken.stderr)
            self.assertIn("Conflicting values set for option Check-Valid-Until", broken.stderr)
            # distrobuilder's special sources.list name replaces the baseline file.
            duplicate.unlink()
            baseline.write_text(sources)
            fixed = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(fixed.returncode, 0, fixed.stderr)
            self.assertNotIn("Conflicting values", fixed.stderr)

    def test_every_image_action_has_valid_bash_syntax(self):
        image = self.text("config/incus/labby-image.yaml")
        actions = image.split("\nactions:\n", 1)[1].split("\nfiles:\n", 1)[0]
        lines = actions.splitlines()
        triggers = [line for line in lines if line.startswith("  - trigger:")]
        scripts = []
        for index, line in enumerate(lines):
            if line.startswith("    action:"):
                self.assertEqual(line, "    action: |-")
                body = []
                for content in lines[index + 1:]:
                    if content and not content.startswith("      "):
                        break
                    body.append(content[6:] if content else "")
                scripts.append("\n".join(body) + "\n")
        self.assertGreater(len(triggers), 0, "the image must contain provision actions")
        self.assertEqual(len(scripts), len(triggers), "every action must be syntax checked")
        self.assertTrue(any("# LABBY_PROVISION_ACTION: crgx" in script for script in scripts))
        bash = shutil.which("bash")
        self.assertIsNotNone(bash, "bash is required to validate image actions")
        # bash -n parses only: it must never execute provisioning or downloads.
        env = {key: value for key, value in os.environ.items()
               if key not in ("BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS")}
        for index, script in enumerate(scripts):
            marker = re.search(r"^# LABBY_(?:PROVISION|IMAGE)_ACTION: (.+)$", script, re.MULTILINE)
            self.assertIsNotNone(marker, f"missing action identity in script {index}")
            with self.subTest(action=marker.group(1)):
                self.assertTrue(script.startswith("#!/usr/bin/env bash\n"))
                result = subprocess.run([bash, "--noprofile", "--norc", "-n"],
                                        input=script, env=env, capture_output=True,
                                        text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_tailscale_consumers_pass_pinned_version_to_installer(self):
        image = self.text("config/incus/labby-image.yaml")
        install_lines = [line.strip() for line in image.splitlines()
                         if 'sh "$tmp/install.sh"' in line]
        self.assertEqual(len(install_lines), 1)
        bootstrap = self.text("scripts/incus-bootstrap.sh")
        self.assertIn('TAILSCALE_INSTALL_VERSION="1.102.3"', bootstrap)
        self.assertIn('run incus exec "$NAME" -- env TAILSCALE_VERSION="$TAILSCALE_INSTALL_VERSION" sh /tmp/labby-tailscale-install.sh', bootstrap)
        # Execute only the image's installer invocation against a shell-function
        # stub: observe its environment without running the downloaded installer.
        env = {key: value for key, value in os.environ.items()
               if key not in ("TAILSCALE_VERSION", "BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS")}
        harness = ("tmp=/fixture\n"
                   "sh() { printf '%s\\n' \"${TAILSCALE_VERSION-unset}\" \"$@\"; }\n"
                   + install_lines[0] + "\n")
        result = subprocess.run(["bash", "--noprofile", "--norc", "-c", harness],
                                env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["1.102.3", "/fixture/install.sh"])

    def test_image_preflight_lint_runs_before_release(self):
        command = "shellcheck scripts/incus-bootstrap.sh scripts/ci/build-incus-image.sh scripts/ci/smoke-incus-image.sh"
        ci = self.text(".github/workflows/ci.yml")
        incus_job = ci.split("  incus-contract:", 1)[1].split("\n  desktop-web:", 1)[0]
        self.assertIn(command, incus_job)
        self.assertIn(command, self.text(".github/workflows/build-incus-image.yml"))

    def test_mise_installer_uses_versioned_release(self):
        image = self.text("config/incus/labby-image.yaml")
        self.assertNotIn("https://mise.run", image)
        self.assertIn("https://github.com/jdx/mise/releases/download/v2026.9.1/install.sh", image)

    def test_operator_install_guidance_never_executes_mutable_urls(self):
        paths = ["README.md", "docs/PLUGINS.md", "docs/runtime/INCUS.md", "scripts/install.sh"]
        for path in paths:
            with self.subTest(path=path):
                text = self.text(path)
                self.assertNotRegex(text, r"raw\.githubusercontent\.com/[^\s]+/(?:main|master)/(?:scripts/)?install\.(?:sh|ps1)")
                self.assertNotRegex(text, r"curl[^\n]+\|\s*(?:ba)?sh")
        release = self.text(".github/workflows/release.yml")
        self.assertIn("labby-install.sh.sha256", release)
        self.assertRegex(release, r"subject-path:[\s\S]+labby-install\.sh")
        bootstrap = self.text("scripts/incus-bootstrap.sh")
        self.assertNotRegex(bootstrap, r"curl[^\n]+\|\s*(?:ba)?sh")
        bootstrap_download = next(line for line in bootstrap.splitlines() if "curl -fsSL" in line)
        self.assertIn("--connect-timeout", bootstrap_download)
        self.assertIn("--max-time", bootstrap_download)
        supply = json.loads(self.text("config/incus/provision-supply.json"))["tailscale_installer"]
        self.assertIn(supply["version"], bootstrap)
        self.assertIn(supply["sha256"], bootstrap)
        readiness = next(line for line in bootstrap.splitlines() if 'incus exec "$NAME" -- curl -fsS' in line and ">/dev/null" in line)
        self.assertIn("--connect-timeout", readiness)
        self.assertIn("--max-time", readiness)

        installer = self.text("scripts/install.sh")
        for line in installer.splitlines():
            if "curl -fsSL" in line:
                self.assertIn("--connect-timeout", line)
                self.assertIn("--max-time", line)
        windows = self.text("scripts/install.ps1")
        for line in windows.splitlines():
            if "Invoke-RestMethod" in line or "Invoke-WebRequest" in line:
                self.assertIn("-TimeoutSec", line)
        windows_ci = self.text(".github/workflows/ci.yml")
        for line in windows_ci.splitlines():
            if "Invoke-WebRequest" in line:
                self.assertIn("-TimeoutSec", line)

        cargo_deny_download = windows_ci[
            windows_ci.index("curl --fail", windows_ci.index("Run pinned Cargo Deny")):
            windows_ci.index("printf '%s  %s", windows_ci.index("Run pinned Cargo Deny"))
        ]
        self.assertIn("--connect-timeout", cargo_deny_download)
        self.assertIn("--max-time", cargo_deny_download)

        incus_smoke = self.text("scripts/ci/smoke-incus-image.sh")
        readiness_probes = [
            line for line in incus_smoke.splitlines()
            if "curl -fsS" in line and "/ready" in line
        ]
        self.assertEqual(len(readiness_probes), 2)
        for probe in readiness_probes:
            self.assertIn("--connect-timeout", probe)
            self.assertIn("--max-time", probe)

    def test_every_incus_supply_pin_is_bound_into_the_image_definition(self):
        image = self.text("config/incus/labby-image.yaml")
        supply = json.loads(self.text("config/incus/provision-supply.json"))
        for name, item in supply.items():
            with self.subTest(name=name):
                self.assertIn(item["version"], image)
                proof = item.get("sha256") or item.get("integrity")
                if proof:
                    self.assertIn(proof, image)
        self.assertNotIn("latest-v", image)
        self.assertNotIn("?mode=json", image)
        self.assertNotRegex(image, r"curl[^\n]+\|\s*(?:ba)?sh")

        workflow = self.text(".github/workflows/build-incus-image.yml")
        distrobuilder_download = workflow[workflow.index("curl --fail"):workflow.index("printf '%s  %s")]
        self.assertIn("--connect-timeout", distrobuilder_download)
        self.assertIn("--max-time", distrobuilder_download)

    def test_mutated_supply_classes_fail_binding(self):
        image = self.text("config/incus/labby-image.yaml")
        supply = json.loads(self.text("config/incus/provision-supply.json"))
        for name, item in supply.items():
            mutated = dict(item)
            field = "sha256" if "sha256" in item else "integrity" if "integrity" in item else "version"
            mutated[field] = "0" * 64 if field == "sha256" else "mutated"
            with self.subTest(name=name):
                self.assertNotIn(mutated[field], image)

    def test_supply_validator_rejects_real_mutations_and_is_cache_independent(self):
        validator = ROOT / "scripts/ci/validate-supply-manifest.py"
        identities = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                for path in ["config/container-supply.conf", "config/Dockerfile", "config/Dockerfile.fast", "config/incus/provision-supply.json", "config/incus/labby-image.yaml"]:
                    destination = root / path
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(ROOT / path, destination)
                result = subprocess.run([validator, "--root", root, "--emit-identity"], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                identities.append(result.stdout.strip())
        self.assertEqual(identities[0], identities[1])

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for path in ["config/container-supply.conf", "config/Dockerfile", "config/Dockerfile.fast", "config/incus/provision-supply.json", "config/incus/labby-image.yaml"]:
                destination = root / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / path, destination)
            manifest = root / "config/incus/provision-supply.json"
            manifest.write_text(manifest.read_text().replace("077e1a0777", "0000000000", 1))
            result = subprocess.run([validator, "--root", root], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for path in ["config/container-supply.conf", "config/Dockerfile", "config/Dockerfile.fast", "config/incus/provision-supply.json", "config/incus/labby-image.yaml"]:
                destination = root / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / path, destination)
            manifest = root / "config/incus/provision-supply.json"
            supply = json.loads(manifest.read_text())
            supply["node"]["version"] = supply["uv"]["version"]
            supply["node"]["sha256"] = supply["uv"]["sha256"]
            manifest.write_text(json.dumps(supply))
            result = subprocess.run([validator, "--root", root], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, "cross-bound Incus supply was accepted")

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for path in ["config/container-supply.conf", "config/Dockerfile", "config/Dockerfile.fast", "config/incus/provision-supply.json", "config/incus/labby-image.yaml"]:
                destination = root / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / path, destination)
            manifest = root / "config/incus/provision-supply.json"
            supply = json.loads(manifest.read_text())
            supply["rust"], supply["go"] = supply["go"], supply["rust"]
            manifest.write_text(json.dumps(supply))
            result = subprocess.run([validator, "--root", root], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, "whole rust/go supply objects were cross-bound")

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for path in ["config/container-supply.conf", "config/Dockerfile", "config/incus/provision-supply.json", "config/incus/labby-image.yaml"]:
                destination = root / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / path, destination)
            dockerfile = root / "config/Dockerfile"
            dockerfile.write_text(dockerfile.read_text().replace("ARG LABBY_NPM_VERSION", "ARG LABBY_UNMANIFESTED_VERSION", 1))
            result = subprocess.run([validator, "--root", root], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, "unmanifested Docker supply was accepted")

    def test_incus_pointer_uses_one_leased_generation_manifest(self):
        text = self.text("scripts/ci/promote-incus-pointer.sh")
        self.assertIn("generation.json", text)
        self.assertIn("--force-with-lease", text)
        self.assertNotIn("git push -f ", text)
        self.assertNotIn('release upload "$rolling_tag"', text)
        self.assertIn('release upload "$release_tag"', text)
        self.assertIn("ls-remote", text)
        self.assertIn('write_state "prepared"', text)
        self.assertIn('write_state "promoted"', text)

    def test_incus_pointer_receipt_drives_leased_rollback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            bin_dir = root / "bin"
            fixture = root / "fixture"
            receipt = root / "receipt"
            bin_dir.mkdir()
            fixture.mkdir()
            artifact = fixture / "labby-incus.tar.gz"
            sbom = fixture / "labby-incus.spdx.json"
            artifact.write_bytes(b"incus-image")
            sbom.write_bytes(b"{}")
            digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
            manifest = {
                "subjects": [{
                    "name": artifact.name, "size": artifact.stat().st_size,
                    "sha256": digest(artifact),
                    "sbom": {"name": sbom.name, "size": sbom.stat().st_size, "sha256": digest(sbom)},
                }],
                "distributions": {"incus": {"asset": artifact.name, "sha256": digest(artifact)}},
            }
            (fixture / "release-manifest.json").write_text(json.dumps(manifest))
            remote = root / "remote"
            local = root / "local"
            remote.write_text("a" * 40)
            git = bin_dir / "git"
            git.write_text("""#!/usr/bin/env bash
set -euo pipefail
case $1 in
  ls-remote) test ! -s "$FAKE_REMOTE" || printf '%s\\trefs/tags/labby-incus-latest\\n' "$(<"$FAKE_REMOTE")" ;;
  tag) printf '%s\\n' "$4" >"$FAKE_LOCAL" ;;
  push)
    expected=; ref=${!#}
    for arg in "$@"; do case $arg in --force-with-lease=*) expected=${arg##*:};; esac; done
    current=$(cat "$FAKE_REMOTE")
    test "$current" = "$expected"
    if [[ $ref == :* ]]; then : >"$FAKE_REMOTE"; else cp "$FAKE_LOCAL" "$FAKE_REMOTE"; fi ;;
  *) exit 64 ;;
esac
""")
            git.chmod(0o755)
            gh = bin_dir / "gh"
            gh.write_text("""#!/usr/bin/env bash
set -euo pipefail
if [[ $1 == release && $2 == download ]]; then
  for arg in "$@"; do [[ $arg != generation.json ]] || exit 1; done
  while (($#)); do [[ $1 != --dir ]] || { cp "$FAKE_FIXTURE"/* "$2/"; exit 0; }; shift; done
elif [[ $1 == release && $2 == upload ]]; then
  exit 0
fi
exit 64
""")
            gh.chmod(0o755)
            env = {
                **os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}",
                "FAKE_REMOTE": str(remote), "FAKE_LOCAL": str(local),
                "FAKE_FIXTURE": str(fixture), "GH_BIN": str(gh), "GH_TOKEN": "test",
                "GITHUB_REPOSITORY": "example/labby", "GITHUB_SHA": "b" * 40,
                "RELEASE_TAG": "v1.2.3", "INCUS_POINTER_RECEIPT": str(receipt),
            }
            script = ROOT / "scripts/ci/promote-incus-pointer.sh"
            subprocess.run([script, "promote"], env=env, check=True)
            self.assertEqual((receipt / "state").read_text().strip(), "promoted")
            self.assertEqual(remote.read_text().strip(), "b" * 40)
            subprocess.run([script, "rollback"], env=env, check=True)
            self.assertEqual((receipt / "state").read_text().strip(), "rolled-back")
            self.assertEqual(remote.read_text().strip(), "a" * 40)

            (receipt / "state").write_text("prepared\n")
            subprocess.run([script, "rollback"], env=env, check=True)
            self.assertEqual(remote.read_text().strip(), "a" * 40)

            # A crash after the CAS but before the final receipt write must
            # still be recognized as a partial promotion and rolled back.
            remote.write_text("b" * 40)
            (receipt / "state").write_text("prepared\n")
            subprocess.run([script, "rollback"], env=env, check=True)
            self.assertEqual(remote.read_text().strip(), "a" * 40)

    def test_incus_supply_downloads_have_connection_and_total_deadlines(self):
        text = self.text("config/incus/labby-image.yaml")
        downloads = [
            line for line in text.splitlines()
            if re.search(r"(?:^|&&\s+)curl\s+-", line.strip())
        ]
        self.assertGreater(len(downloads), 0)
        for line in downloads:
            self.assertIn("--connect-timeout", line)
            self.assertIn("--max-time", line)


if __name__ == "__main__":
    unittest.main()
