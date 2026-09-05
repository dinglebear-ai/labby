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


class ContainerTopology(unittest.TestCase):
    def text(self, path):
        return (ROOT / path).read_text()

    def test_compose_base_is_minimal_and_recoverable(self):
        text = self.text("docker-compose.prod.yml")
        self.assertNotIn("container_name:", text)
        self.assertRegex(text, r"image:.*sha256")
        for contract in ["127.0.0.1:", "read_only: true", "cap_drop: [ALL]", "no-new-privileges:true", "max-size", "max-file", "healthcheck:"]:
            self.assertIn(contract, text)
        self.assertNotRegex(text, r"\$\{?HOME\}?\s*:\s*/home")
        self.assertIn("profiles: [integrations]", text)
        self.assertIn("labby-master:", text)

    def test_immutable_image_preflight_rejects_tags(self):
        script = ROOT / "scripts/ci/validate-container-inputs.sh"
        env = {**os.environ, "LABBY_IMAGE": "ghcr.io/dinglebear-ai/labby:latest"}
        self.assertNotEqual(subprocess.run([script], env=env, check=False).returncode, 0)
        digest = "a" * 64
        env.update({
            "LABBY_IMAGE": f"ghcr.io/dinglebear-ai/labby@sha256:{digest}",
            "LABBY_BUILDER_IMAGE": f"docker.io/library/rust@sha256:{digest}",
            "LABBY_RUNTIME_IMAGE": f"docker.io/library/debian@sha256:{digest}",
        })
        self.assertEqual(subprocess.run([script], env=env, check=False).returncode, 0)

    def test_dev_compose_retains_production_safety_controls(self):
        text = self.text("docker-compose.yml")
        for contract in ["127.0.0.1:", "read_only: true", "cap_drop: [ALL]", "no-new-privileges:true"]:
            self.assertIn(contract, text)

        dockerfile = self.text("config/Dockerfile.fast")
        supply = self.text("config/container-supply.conf")
        for key in ["LABBY_RUNTIME_IMAGE", "LABBY_DEBIAN_SNAPSHOT", "LABBY_NODE_VERSION", "LABBY_NODE_SHA256", "LABBY_UV_VERSION", "LABBY_UV_SHA256", "LABBY_AGENT_CLIS_LOCK_SHA256"]:
            value = re.search(rf"(?m)^{key}=(\S+)$", supply).group(1)
            self.assertIn(f"ARG {key}={value}", dockerfile)
            self.assertIn(f"${{{key}}}", dockerfile)
        self.assertNotRegex(dockerfile, r"curl[^\n]+\|\s*(?:ba)?sh")
        self.assertNotIn("npm install", dockerfile)
        for line in dockerfile.splitlines():
            if re.search(r"(?:^|&&\s+)curl\s+-", line.strip()):
                self.assertIn("--connect-timeout", line)
                self.assertIn("--max-time", line)

    def test_dev_compose_renders_without_production_environment(self):
        env = dict(os.environ)
        env.pop("LABBY_IMAGE", None)
        env.pop("LABBY_CONFIG_DIR", None)
        rendered = subprocess.run(
            ["docker", "compose", "-f", str(ROOT / "docker-compose.yml"), "config"],
            cwd=ROOT,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(rendered.returncode, 0, rendered.stderr)
        self.assertIn("/workspace/lab", rendered.stdout)
        self.assertIn("host.docker.internal", rendered.stdout)
        self.assertIn("axon: null", rendered.stdout)
        self.assertIn("lab: null", rendered.stdout)

    def test_production_launcher_rejects_mutable_image_before_compose(self):
        launcher = ROOT / "scripts/run-compose-prod.sh"
        env = {
            **os.environ,
            "LABBY_IMAGE": "ghcr.io/dinglebear-ai/labby:latest",
            "LABBY_CONFIG_DIR": str(ROOT / "config"),
        }
        result = subprocess.run(
            [launcher, "config", "--quiet"],
            cwd=ROOT,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 64)
        self.assertIn("immutable", result.stderr)

    def test_dockerfile_requires_immutable_base_inputs_and_recovery_evidence(self):
        text = self.text("config/Dockerfile")
        healthcheck = self.text("scripts/ci/container-healthcheck.sh")
        self.assertIn("ARG LABBY_BUILDER_IMAGE", text)
        self.assertIn("FROM ${LABBY_BUILDER_IMAGE}", text)
        self.assertIn("ARG LABBY_RUNTIME_IMAGE", text)
        self.assertIn("FROM ${LABBY_RUNTIME_IMAGE}", text)
        self.assertIn("labby-healthcheck", text)
        self.assertIn("/home/labby/.local/state/labby", healthcheck)
        self.assertIn("health-recovery.log", healthcheck)
        self.assertIn("recovery exhausted", healthcheck)
        self.assertIn('sleep "$delay"', healthcheck)
        self.assertNotIn("TODO: pin by digest", text)
        supply = self.text("config/container-supply.conf")
        self.assertRegex(supply, r"LABBY_BUILDER_IMAGE=.*@sha256:[0-9a-f]{64}")
        self.assertRegex(supply, r"LABBY_RUNTIME_IMAGE=.*@sha256:[0-9a-f]{64}")
        for key in ["LABBY_DEBIAN_SNAPSHOT", "LABBY_NODE_VERSION", "LABBY_NODE_SHA256", "LABBY_UV_VERSION", "LABBY_UV_SHA256", "LABBY_AGENT_CLIS_LOCK_SHA256"]:
            self.assertRegex(supply, rf"(?m)^{key}=\S+$")
            self.assertIn(f"${{{key}}}", text)
        self.assertTrue((ROOT / "config/agent-clis/package-lock.json").is_file())
        self.assertIn("COPY config/agent-clis/package", text)
        self.assertIn("npm ci", text)
        for line in text.splitlines():
            if re.search(r"(?:^|&&\s+)curl\s+-", line.strip()):
                self.assertIn("--connect-timeout", line)
                self.assertIn("--max-time", line)
        self.assertNotIn("npm install --omit=dev", text)
        release = self.text(".github/workflows/release.yml")
        self.assertIn("source config/container-supply.conf", release)
        self.assertIn("LABBY_BUILDER_IMAGE=${{ steps.container_supply.outputs.builder }}", release)

    def test_incus_sources_are_https(self):
        text = self.text("config/incus/labby-image.yaml")
        self.assertNotIn("url: http://", text)
        self.assertNotIn("mirror: http://", text)
        self.assertIn("https://snapshot.ubuntu.com/ubuntu/", text)
        self.assertNotIn('uv" python install', text)

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

    def test_compose_n_minus_one_adapter_is_complete(self):
        path = ROOT / "scripts/ci/n-minus-one/compose"
        self.assertTrue(path.stat().st_mode & stat.S_IXUSR)
        text = path.read_text()
        for stage in ["install-previous", "seed-state", "verify-previous", "verify-provenance", "upgrade", "verify-candidate", "authenticated-action", "restart", "verify-restart", "rollback", "verify-rollback"]:
            self.assertIn(stage, text)
        self.assertLess(text.index("verify-provenance)"), text.index("upgrade)"))
        self.assertIn("docker-compose.prod.yml", text)
        self.assertIn("trap cleanup EXIT", text)
        self.assertNotIn('cat >"$base"', text)
        self.assertNotIn(":/usr/local/bin/labby", text)
        self.assertIn("LABBY_N_MINUS_ONE_PREVIOUS_IMAGE", text)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_IMAGE", text)

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

    def test_operator_qualification_spans_external_route_and_recovery(self):
        text = self.text("scripts/ci/qualify-container-operator.sh")
        for contract in [
            "https://", "--cacert", "oauth-protected-resource", "resource",
            "Authorization: Bearer", "LABBY_QUALIFY_UPSTREAM_SERVICE",
            "snippets.create", "snippets.get", "LABBY_QUALIFY_RESTART",
            "snippets.remove", "LABBY_QUALIFY_BACKUP_OBSERVER", "backup_before", "backup_after",
            'run_observer "$backup" create', 'run_observer "$backup" contains',
            'run_observer "$backup" restore', 'run_observer "$restart"',
            "--connect-timeout", "--max-time", "timeout=timeout",
        ]:
            self.assertIn(contract, text)
        self.assertNotIn("eval ", text)

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

    def test_operator_qualification_bounds_a_hanging_observer(self):
        with tempfile.TemporaryDirectory() as raw:
            temp = pathlib.Path(raw)
            curl = temp / "curl"
            curl.write_text("""#!/bin/sh
case "$*" in
  *oauth-protected-resource*) printf '{"resource":"https://operator.example"}' ;;
  *--write-out*) printf '401' ;;
  *) printf '{"ok":true,"value":"durable"}' ;;
esac
""")
            observer = temp / "observer"
            observer.write_text("#!/bin/sh\nsleep 10\n")
            restart = temp / "restart"
            restart.write_text("#!/bin/sh\nexit 0\n")
            for executable in [curl, observer, restart]:
                executable.chmod(0o755)
            ca = temp / "ca.pem"
            ca.write_text("fixture")
            env = {
                **os.environ,
                "PATH": f"{temp}:{os.environ['PATH']}",
                "LABBY_QUALIFY_BASE_URL": "https://operator.example",
                "LABBY_QUALIFY_TOKEN": "fixture",
                "LABBY_QUALIFY_CA_CERT": str(ca),
                "LABBY_QUALIFY_RESOURCE_ROOT": "https://operator.example",
                "LABBY_QUALIFY_UPSTREAM_SERVICE": "gateway",
                "LABBY_QUALIFY_UPSTREAM_ACTION": "gateway.status",
                "LABBY_QUALIFY_RESTART": str(restart),
                "LABBY_QUALIFY_BACKUP_OBSERVER": str(observer),
                "LABBY_QUALIFY_OBSERVER_TIMEOUT_SECONDS": "0.1",
            }
            started = time.monotonic()
            result = subprocess.run(
                [ROOT / "scripts/ci/qualify-container-operator.sh"],
                env=env, capture_output=True, text=True, check=False,
            )
            self.assertLess(time.monotonic() - started, 2)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("observer timed out", result.stderr)


if __name__ == "__main__":
    unittest.main()
