#!/usr/bin/env python3
"""Executable contracts for release hardening and publication recovery."""

from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
import yaml


ROOT = Path(__file__).resolve().parents[2]


class ReleaseWorkflowContractTests(unittest.TestCase):
    def text(self, relative: str) -> str:
        return (ROOT / relative).read_text()

    def test_release_candidates_are_qualified_before_publication(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("push:\n    tags: ['v*']", workflow)
        self.assertNotIn("types: [published]", workflow)
        qualify = workflow.index("name: Qualify complete release candidate")
        publish = workflow.index("name: Publish qualified draft release")
        self.assertLess(qualify, publish)
        self.assertIn("scripts/ci/promote-release.sh", workflow[publish:])
        self.assertIn('gh release edit "$tag" --draft=false', self.text("scripts/ci/promote-release.sh"))

    def test_promotion_has_tag_env_precedes_no_publisher_and_is_recovery_guarded(self) -> None:
        workflow = yaml.load(self.text(".github/workflows/release.yml"), Loader=yaml.BaseLoader)
        release_steps = workflow["jobs"]["release"]["steps"]
        by_name = {step.get("name"): (index, step) for index, step in enumerate(release_steps)}
        promote_index, promote = by_name["Publish qualified draft release"]
        self.assertEqual("${{ github.ref_name }}", promote.get("env", {}).get("RELEASE_TAG"))
        self.assertIn("scripts/ci/promote-release.sh", promote["run"])
        for name in ("Publish exact tested image", "Publish validated npm launcher"):
            self.assertLess(by_name[name][0], promote_index)
        rollback_index = by_name["Roll back partial publication"][0]
        self.assertGreater(rollback_index, promote_index)
        self.assertEqual("${{ failure() }}", release_steps[rollback_index]["if"])
        self.assertEqual("${{ steps.publication.outputs.started }}", release_steps[rollback_index]["env"]["IMAGE_PUBLICATION_STARTED"])

    def test_release_workflow_generates_one_sbom_per_subject(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("scripts/ci/generate-release-sboms.sh", workflow)
        self.assertIn("lab-*.spdx.json", workflow)
        self.assertIn("lab-container-image.spdx.json", workflow)
        self.assertNotIn("output-file: labby.spdx.json", workflow)

    def test_failed_promotion_redrafts_release_and_records_one_compound_result(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        rollback = workflow[workflow.index("      - name: Roll back partial publication"):]
        self.assertIn('gh release edit "$RELEASE_TAG" --draft=true', rollback)
        compound = self.text("scripts/ci/compound-release-rollback.py")
        for field in ("github_release", "incus_pointer", "image_registry"):
            self.assertIn(field, compound)
        self.assertIn("compound-release-rollback.py", rollback)

    def test_release_consumers_verify_provenance_before_activation(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("scripts/ci/verify-release-provenance.sh", workflow)
        self.assertIn("--repo ${{ github.repository }}", workflow)
        self.assertIn("--workflow release.yml", workflow)
        self.assertIn("--ref ${{ github.ref }}", workflow)
        helper = self.text("scripts/ci/verify-and-activate-release.sh")
        self.assertLess(helper.index("verify-release-provenance.sh"), helper.index('exec "$@"'))
        unix_installer = self.text("scripts/install.sh")
        self.assertLess(unix_installer.index("gh attestation verify"), unix_installer.index("tar -xzf"))
        windows_installer = self.text("scripts/install.ps1")
        self.assertLess(windows_installer.index("gh attestation verify"), windows_installer.index("Expand-Archive"))

    def test_release_has_executable_n_minus_one_upgrade_qualification(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("name: N-1 stateful upgrade and rollback qualification", workflow)
        self.assertIn("scripts/ci/qualify-n-minus-one.sh", workflow)
        for deployment in ("unix", "windows", "macos", "compose", "incus", "host-service"):
            self.assertIn(deployment, workflow)

    def test_release_has_machine_readable_manifest_and_reconciler(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        reminder = self.text(".github/workflows/release-publish-reminder.yml")
        self.assertIn("scripts/ci/create-release-manifest.py", workflow)
        self.assertIn("release-manifest.json", workflow)
        self.assertIn("scripts/ci/reconcile-release.py", reminder)
        self.assertIn("manage-release-incident.sh", reminder)
        self.assertIn("Release publication is incomplete", self.text("scripts/ci/manage-release-incident.sh"))
        for surface in ("github", "npm", "ghcr", "incus", "mcp"):
            self.assertIn(f'"{surface}"', self.text("scripts/ci/reconcile-release.py"))

    def test_candidate_publishers_are_called_before_stable_promotion(self) -> None:
        release = yaml.load(self.text(".github/workflows/release.yml"), Loader=yaml.BaseLoader)
        jobs = release["jobs"]
        self.assertEqual("./.github/workflows/build-incus-image.yml", jobs["incus-candidate"]["uses"])
        self.assertEqual("./.github/workflows/mcp-registry.yml", jobs["mcp-candidate"]["uses"])
        self.assertIn("incus-candidate", jobs["release"]["needs"])
        self.assertIn("mcp-candidate", jobs["release"]["needs"])
        for path in (".github/workflows/build-incus-image.yml", ".github/workflows/mcp-registry.yml"):
            text = self.text(path)
            self.assertIn("workflow_call:", text)
            self.assertNotIn("types: [published]", text)

    def test_incus_candidate_is_immutable_and_pointer_is_transactional(self) -> None:
        release = yaml.load(self.text(".github/workflows/release.yml"), Loader=yaml.BaseLoader)
        self.assertEqual("write", release["jobs"]["incus-candidate"]["permissions"]["contents"])
        incus = self.text(".github/workflows/build-incus-image.yml")
        self.assertNotIn("ROLLING_TAG", incus)
        self.assertNotIn("git push -f", incus)
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("scripts/ci/promote-incus-pointer.sh promote", workflow)
        self.assertIn("scripts/ci/promote-incus-pointer.sh rollback", workflow)

    def test_immutable_release_assets_are_never_clobbered(self) -> None:
        for path in (".github/workflows/release.yml", ".github/workflows/build-incus-image.yml"):
            self.assertNotIn("--clobber", self.text(path), path)
        self.assertIn("upload-immutable-release-assets.sh", self.text(".github/workflows/release.yml"))

    def test_mcp_observer_never_copies_expected_digest(self) -> None:
        observer = self.text("scripts/ci/observe-release.py")
        self.assertNotIn('observed["mcp"] = dist["mcp"]', observer)
        self.assertIn("hashlib.sha256(canonical).hexdigest()", observer)

    def test_lifecycle_inventory_routes_every_script_and_public_copy(self) -> None:
        inventory = json.loads(self.text("scripts/ci/lifecycle-scripts.json"))
        paths = inventory["shell"] + inventory["powershell"] + inventory["public_copies"]
        self.assertEqual(len(paths), len(set(paths)))
        for path in paths:
            self.assertTrue((ROOT / path).is_file(), path)
        checker = self.text("scripts/ci/check-lifecycle-scripts.sh")
        self.assertIn("lifecycle-scripts.json", checker)
        classifier = self.text("scripts/ci/changed_paths.py")
        for path in ("scripts/ci/lifecycle-scripts.json", "scripts/ci/check-lifecycle-scripts.sh", "scripts/ci/test_release_hardening.py"):
            self.assertIn(path, classifier)
        for path in paths:
            with tempfile.TemporaryDirectory() as tmp:
                changed = Path(tmp) / "changed"; output = Path(tmp) / "output"
                changed.write_text(path + "\n")
                subprocess.run(["python3", str(ROOT / "scripts/ci/changed_paths.py"), "--event", "pull_request", "--changed-files", str(changed), "--output", str(output)], check=True, stdout=subprocess.DEVNULL)
                routed = dict(line.split("=", 1) for line in output.read_text().splitlines())
                self.assertEqual("true", routed["workflow"], path)
                self.assertEqual("true", routed["docker"], path)

    def test_lifecycle_inventory_is_self_reconciling(self) -> None:
        checker = self.text("scripts/ci/check-lifecycle-scripts.sh")
        self.assertIn("untracked lifecycle shell entrypoint", checker)
        self.assertIn("untracked lifecycle PowerShell entrypoint", checker)
        self.assertIn("-name '*.ps1'", checker)
        self.assertIn("rglob", checker)

    def test_compose_activation_verifies_image_attestation_identity(self) -> None:
        launcher = self.text("scripts/run-compose-prod.sh")
        for value in ("gh attestation verify", "--signer-workflow", "--source-ref", "--deny-self-hosted-runners"):
            self.assertIn(value, launcher)
        self.assertLess(launcher.index("gh attestation verify"), launcher.index("exec docker compose"))

    def test_release_set_keeps_older_incomplete_version_failed(self) -> None:
        helper = ROOT / "scripts/ci/reconcile-release-set.py"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for tag, complete in (("v1.0.0", False), ("v1.1.0", True)):
                directory = root / tag
                directory.mkdir()
                (directory / "reconciliation.json").write_text(json.dumps({"complete": complete}))
            output = root / "aggregate.json"
            result = subprocess.run(["python3", str(helper), "--reports", str(root), "--output", str(output)], check=False)
            self.assertNotEqual(0, result.returncode)
            reports = json.loads(output.read_text())["versions"]
            self.assertEqual(["v1.0.0", "v1.1.0"], [row["tag"] for row in reports])

    def test_irreversible_publications_remain_visible_and_stable_pointer_is_last(self) -> None:
        release = self.text(".github/workflows/release.yml")
        candidate = release.index('npm publish --access public --tag "candidate-$version"')
        promote = release.index("scripts/ci/promote-release.sh")
        stable = release.index('npm dist-tag add "$package_name@$version" latest')
        self.assertLess(candidate, promote)
        self.assertLess(promote, stable)
        reminder = self.text(".github/workflows/release-publish-reminder.yml")
        self.assertIn("[.tag_name, .draft] | @tsv", reminder)
        self.assertIn("draft release is missing release-manifest.json", reminder)

    def test_n_minus_one_uses_real_runtime_schema_not_probe_tables(self) -> None:
        helper = self.text("scripts/ci/n-minus-one-durable-state.py")
        self.assertNotIn("n1_compatibility_probe", helper)
        for table in ("registered_clients", "access_security_events", "upstream_calls"):
            self.assertIn(table, helper)

    def test_shared_cache_policy_is_fail_closed_and_ref_scoped(self) -> None:
        workflow = self.text(".github/workflows/ci.yml")
        action = self.text(".github/actions/setup-rust-kache/action.yml")
        self.assertIn("scripts/ci/check-cache-boundary.py", workflow)
        self.assertIn("github.run_id", workflow)
        self.assertIn("s3-prefix: ${{ env.KACHE_S3_PREFIX }}/${{ github.run_id }}", action)
        self.assertNotIn("KACHE_S3_ACCESS_KEY: ${{ secrets.KACHE_S3_ACCESS_KEY }}", workflow)
        self.assertIn("KACHE_S3_PREFIX_ENFORCED", workflow)
        self.assertIn("KACHE_S3_PREFIX_ENFORCED", self.text("scripts/ci/check-cache-boundary.py"))

    def test_pull_request_workflow_has_no_repository_cache_secret_reference(self) -> None:
        workflows = "\n".join(path.read_text() for path in (ROOT / ".github/workflows").glob("*.yml"))
        self.assertNotIn("secrets.KACHE_S3_ACCESS_KEY", workflows)
        self.assertNotIn("secrets.KACHE_S3_SECRET_KEY", workflows)

    def test_pull_request_jobs_do_not_receive_shared_kache_credentials(self) -> None:
        workflow = self.text(".github/workflows/ci.yml")
        header = workflow[: workflow.index("jobs:")]
        self.assertIn('KACHE_S3_ACCESS_KEY: ""', header)
        self.assertIn('KACHE_S3_SECRET_KEY: ""', header)
        self.assertNotIn("secrets.KACHE_S3_", workflow)
        self.assertIn("github.event_name != 'pull_request'", workflow)

    def test_ci_runs_centralized_lifecycle_static_analysis(self) -> None:
        workflow = self.text(".github/workflows/ci.yml")
        self.assertIn("scripts/ci/check-lifecycle-scripts.sh", workflow)
        self.assertIn("PSScriptAnalyzer", workflow)
        self.assertIn("Invoke-ScriptAnalyzer", workflow)

    def test_docker_cache_skeleton_covers_every_explicit_binary(self) -> None:
        dockerfile = self.text("config/Dockerfile")
        manifest = self.text("crates/labby/Cargo.toml")
        paths = []
        in_bin = False
        for line in manifest.splitlines():
            if line == "[[bin]]":
                in_bin = True
            elif line.startswith("["):
                in_bin = False
            elif in_bin and line.startswith("path = "):
                paths.append(line.split('"')[1])
        self.assertGreaterEqual(len(paths), 3)
        for path in paths:
            self.assertIn(f"crates/labby/{path}", dockerfile)


class RollbackTransactionTests(unittest.TestCase):
    def test_rollback_attempts_every_step_for_single_and_joint_failures(self) -> None:
        script = ROOT / "scripts/ci/release-image-rollback.sh"
        for fail_match in ("DELETE", "create", "inspect", "DELETE,create,inspect"):
            with self.subTest(fail_match=fail_match), tempfile.TemporaryDirectory() as tmp:
                work = Path(tmp)
                log = work / "calls"
                fake = work / "fake"
                fake.write_text(
                    "#!/bin/sh\n"
                    "printf '%s\\n' \"$*\" >> \"$CALL_LOG\"\n"
                    "old_ifs=$IFS; IFS=,\n"
                    "for needle in $FAIL_MATCH; do case \" $* \" in *\"$needle\"*) exit 7;; esac; done\n"
                    "IFS=$old_ifs\n"
                    "case \" $* \" in *inspect*) printf '\"%s\"\\n' \"$EXPECTED_DIGEST\";; esac\n"
                )
                fake.chmod(0o755)
                expected = "sha256:" + "a" * 64
                env = os.environ | {
                    "CALL_LOG": str(log),
                    "FAIL_MATCH": fail_match,
                    "EXPECTED_DIGEST": expected,
                    "GH_BIN": str(fake),
                    "DOCKER_BIN": str(fake),
                    "ROLLBACK_VERIFY_ATTEMPTS": "1",
                    "ROLLBACK_VERIFY_DELAY_SECONDS": "0",
                }
                result = subprocess.run(
                    [
                        "bash", str(script), "--image", "ghcr.io/acme/labby",
                        "--tag", "v1.2.3", "--previous-latest", expected,
                        "--delete-version-id", "42",
                    ],
                    text=True, capture_output=True, env=env, check=False,
                )
                self.assertNotEqual(0, result.returncode)
                calls = log.read_text()
                self.assertIn("DELETE", calls)
                self.assertIn("imagetools create", calls)
                self.assertIn("imagetools inspect", calls)
                status = json.loads(result.stdout.splitlines()[-1])
                self.assertEqual("failed", status["status"])
                self.assertEqual({"delete_version", "release_tag_absent", "restore_release", "verify_release", "restore_latest", "verify_latest"}, set(status["steps"]))

    def test_rollback_restores_preexisting_release_tag_instead_of_requiring_absence(self) -> None:
        script = (ROOT / "scripts/ci/release-image-rollback.sh").read_text()
        self.assertIn("--previous-release", script)
        self.assertIn("restore_release", script)
        self.assertIn("verify_release", script)
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("labby-previous-release-digest", workflow)
        self.assertIn('--previous-release "$previous_release"', workflow)
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); log = work / "calls"; fake = work / "fake"
            fake.write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CALL_LOG\"\n"
                "case \" $* \" in\n"
                "*'inspect ghcr.io/acme/labby:v1.2.3 '*) printf '\"%s\"\\n' \"$PREVIOUS_RELEASE\";;\n"
                "*'inspect ghcr.io/acme/labby:latest '*) printf '\"%s\"\\n' \"$PREVIOUS_LATEST\";;\n"
                "esac\n"
            )
            fake.chmod(0o755)
            release = "sha256:" + "a" * 64; latest = "sha256:" + "b" * 64
            env = os.environ | {"CALL_LOG": str(log), "GH_BIN": str(fake), "DOCKER_BIN": str(fake), "PREVIOUS_RELEASE": release, "PREVIOUS_LATEST": latest}
            result = subprocess.run(["bash", str(ROOT / "scripts/ci/release-image-rollback.sh"), "--image", "ghcr.io/acme/labby", "--tag", "v1.2.3", "--previous-release", release, "--previous-latest", latest], env=env, check=False)
            self.assertEqual(0, result.returncode)
            calls = log.read_text()
            self.assertIn(f"imagetools create --tag ghcr.io/acme/labby:v1.2.3 ghcr.io/acme/labby@{release}", calls)
            self.assertIn(f"imagetools create --tag ghcr.io/acme/labby:latest ghcr.io/acme/labby@{latest}", calls)


class ReleaseHelperTests(unittest.TestCase):
    def text(self, relative: str) -> str:
        return (ROOT / relative).read_text()

    def test_immutable_uploader_reuses_equal_bytes_and_rejects_drift(self) -> None:
        helper = ROOT / "scripts/ci/upload-immutable-release-assets.sh"
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); remote = work / "remote"; remote.mkdir(); local = work / "asset"; local.write_text("same")
            gh = work / "gh"
            gh.write_text("#!/bin/sh\nif [ \"$1 $2\" = \"release download\" ]; then cp \"$REMOTE/$5\" \"$7/$5\"; else echo \"$*\" >>\"$CALL_LOG\"; fi\n")
            gh.chmod(0o755); (remote / "asset").write_text("same")
            env = os.environ | {"PATH": f"{work}:{os.environ['PATH']}", "REMOTE": str(remote), "CALL_LOG": str(work / "calls"), "RELEASE_TAG": "v1"}
            self.assertEqual(0, subprocess.run([str(helper), "--", str(local)], env=env).returncode)
            local.write_text("different")
            self.assertEqual(73, subprocess.run([str(helper), "--", str(local)], env=env, stderr=subprocess.DEVNULL).returncode)

    def test_compound_rollback_fails_when_any_surface_fails(self) -> None:
        helper = ROOT / "scripts/ci/compound-release-rollback.py"
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); image = work / "image.json"; output = work / "out.json"
            image.write_text('{"status":"ok"}')
            command = ["python3", str(helper), "--image-record", str(image), "--image-rc", "0", "--pointer-rc", "7", "--github-rc", "0", "--npm-candidate-published", "false", "--mcp-version-published", "false", "--output", str(output)]
            self.assertNotEqual(0, subprocess.run(command, stdout=subprocess.DEVNULL).returncode)
            result = json.loads(output.read_text())
            self.assertEqual("failed", result["status"])
            self.assertEqual("failed", result["incus_pointer"]["status"])
            image.unlink()
            command[command.index("--pointer-rc") + 1] = "0"
            self.assertNotEqual(0, subprocess.run(command, stdout=subprocess.DEVNULL).returncode)

    def test_compound_rollback_never_hides_irreversible_registry_identity(self) -> None:
        helper = ROOT / "scripts/ci/compound-release-rollback.py"
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); image = work / "image.json"; output = work / "out.json"
            image.write_text('{"status":"ok"}')
            command = ["python3", str(helper), "--image-record", str(image), "--image-rc", "0", "--pointer-rc", "0", "--github-rc", "0", "--npm-candidate-published", "true", "--mcp-version-published", "true", "--output", str(output)]
            self.assertNotEqual(0, subprocess.run(command, stdout=subprocess.DEVNULL).returncode)
            result = json.loads(output.read_text())
            self.assertEqual("manual_reconciliation_required", result["npm_candidate"]["status"])
            self.assertEqual("manual_reconciliation_required", result["mcp_version"]["status"])
    def test_release_incident_is_idempotently_created_updated_and_closed(self) -> None:
        script = ROOT / "scripts/ci/manage-release-incident.sh"
        for complete, existing, expected in (
            ("false", "", "issue create"),
            ("false", "17", "issue edit 17"),
            ("true", "17", "issue close 17"),
            ("true", "", "issue list"),
        ):
            with self.subTest(complete=complete, existing=existing), tempfile.TemporaryDirectory() as tmp:
                work = Path(tmp); log = work / "calls"; gh = work / "gh"
                gh.write_text(
                    "#!/bin/sh\n"
                    "printf '%s\\n' \"$*\" >> \"$CALL_LOG\"\n"
                    "case \"$1 $2\" in 'issue list') printf '%s\\n' \"$ISSUE_NUMBER\";; esac\n"
                ); gh.chmod(0o755)
                (work / "reconciliation.json").write_text('{"complete":false}')
                env = os.environ | {"PATH": f"{work}:{os.environ['PATH']}", "CALL_LOG": str(log), "ISSUE_NUMBER": existing, "GITHUB_SERVER_URL": "https://github.test", "GITHUB_REPOSITORY": "acme/labby", "GITHUB_RUN_ID": "1"}
                result = subprocess.run(["bash", str(script), complete], cwd=work, env=env, check=False)
                self.assertEqual(0, result.returncode)
                self.assertIn(expected, log.read_text())

    def test_cache_boundary_denies_pr_credentials_and_partial_capabilities(self) -> None:
        script = str(ROOT / "scripts/ci/check-cache-boundary.py")
        base = ["python3", script, "--event", "pull_request", "--ref", "refs/pull/1/merge", "--run-id", "42"]
        for env in (
            {"KACHE_S3_ACCESS_KEY": "access", "KACHE_S3_SECRET_KEY": "secret"},
            {"KACHE_S3_ACCESS_KEY": "access"},
        ):
            self.assertNotEqual(0, subprocess.run(base, env=os.environ | env, check=False).returncode)
        trusted = ["python3", script, "--event", "push", "--ref", "refs/heads/main", "--run-id", "42"]
        capability = {"KACHE_S3_ACCESS_KEY": "access", "KACHE_S3_SECRET_KEY": "secret", "KACHE_S3_PREFIX": "rust/main"}
        self.assertNotEqual(0, subprocess.run(trusted, env=os.environ | capability, check=False).returncode)
        capability["KACHE_S3_PREFIX_ENFORCED"] = "true"
        self.assertEqual(0, subprocess.run(trusted, env=os.environ | capability, check=False).returncode)

    def test_provenance_verifier_pins_identity_and_offline_trust_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            log = work / "calls"
            gh = work / "gh"
            gh.write_text("#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CALL_LOG\"\n")
            gh.chmod(0o755)
            artifact = work / "artifact"
            bundle = work / "bundle.jsonl"
            root = work / "trusted-root.jsonl"
            for path in (artifact, bundle, root):
                path.write_text("fixture")
            env = os.environ | {"PATH": f"{work}:{os.environ['PATH']}", "CALL_LOG": str(log)}
            result = subprocess.run(
                [
                    "bash", str(ROOT / "scripts/ci/verify-release-provenance.sh"),
                    "--repo", "acme/labby", "--workflow", "release.yml",
                    "--ref", "refs/tags/v1.2.3", "--artifact", str(artifact),
                    "--bundle", str(bundle), "--trusted-root", str(root),
                ], env=env, check=False,
            )
            self.assertEqual(0, result.returncode)
            call = log.read_text()
            self.assertIn("--repo acme/labby", call)
            self.assertIn("--signer-workflow acme/labby/.github/workflows/release.yml", call)
            self.assertIn("--source-ref refs/tags/v1.2.3", call)
            self.assertIn("--deny-self-hosted-runners", call)
            self.assertIn(f"--bundle {bundle}", call)
            self.assertIn(f"--custom-trusted-root {root}", call)

    def test_provenance_verifier_rejects_wrong_online_and_offline_identity(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); artifact = work / "artifact"; artifact.write_text("fixture")
            gh = work / "gh"
            gh.write_text(
                "#!/bin/sh\n"
                "case \" $* \" in\n"
                "*'--repo acme/labby '*'--signer-workflow acme/labby/.github/workflows/release.yml '*'--source-ref refs/tags/v1.2.3 '*) exit 0;;\n"
                "*) exit 9;; esac\n"
            )
            gh.chmod(0o755)
            env = os.environ | {"PATH": f"{work}:{os.environ['PATH']}"}
            base = ["bash", str(ROOT / "scripts/ci/verify-release-provenance.sh"), "--workflow", "release.yml", "--ref", "refs/tags/v1.2.3", "--artifact", str(artifact)]
            self.assertEqual(0, subprocess.run(base + ["--repo", "acme/labby"], env=env, check=False).returncode)
            self.assertNotEqual(0, subprocess.run(base + ["--repo", "evil/labby"], env=env, check=False).returncode)
            bundle = work / "bundle"; root = work / "root"; bundle.write_text("x"); root.write_text("x")
            wrong = base + ["--repo", "evil/labby", "--bundle", str(bundle), "--trusted-root", str(root)]
            self.assertNotEqual(0, subprocess.run(wrong, env=env, check=False).returncode)

    def test_n_minus_one_driver_fails_closed_when_adapter_stage_is_missing(self) -> None:
        result = subprocess.run(
            ["bash", str(ROOT / "scripts/ci/qualify-n-minus-one.sh"), "unix", "v1.0.0", "v1.1.0"],
            text=True,
            capture_output=True,
            env={"PATH": os.environ["PATH"]},
            check=False,
        )
        self.assertNotEqual(0, result.returncode)
        self.assertIn("LABBY_N_MINUS_ONE_INSTALL_PREVIOUS", result.stderr)

    def test_n_minus_one_requires_provenance_authenticated_action_and_restart(self) -> None:
        driver = (ROOT / "scripts/ci/qualify-n-minus-one.sh").read_text()
        for stage in ("verify_provenance", "authenticated_action", "restart", "verify_restart"):
            self.assertIn(stage, driver)

    def test_manifest_and_reconciler_bind_exact_subject_digests(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            subject = work / "lab-linux.tar.gz"
            subject.write_bytes(b"candidate")
            sbom = work / "lab-linux.spdx.json"
            sbom.write_bytes(b"sbom")
            manifest = work / "manifest.json"
            subprocess.run(
                [
                    "python3", str(ROOT / "scripts/ci/create-release-manifest.py"),
                    "--tag", "v1.2.3", "--repository", "acme/labby",
                    "--incus-sha256", "1" * 64, "--mcp-manifest-sha256", "2" * 64,
                    "--output", str(manifest), str(subject), str(sbom),
                ],
                check=True,
            )
            row = json.loads(manifest.read_text())["subjects"][0]
            observed = work / "observed.json"
            manifest_data = json.loads(manifest.read_text())
            verified = [{"subject": item["subject"], "status": "verified"} for item in manifest_data["attestations"]]
            observed.write_text(json.dumps({"subjects": [row, row["sbom"]], "attestations": verified, "distributions": manifest_data["distributions"]}))
            command = ["python3", str(ROOT / "scripts/ci/reconcile-release.py"), "--manifest", str(manifest), "--observed", str(observed)]
            self.assertEqual(0, subprocess.run(command, check=False).returncode)
            row["sha256"] = "0" * 64
            observed.write_text(json.dumps({"subjects": [row, row["sbom"]], "attestations": verified, "distributions": manifest_data["distributions"]}))
            failed = subprocess.run(command, text=True, capture_output=True, check=False)
            self.assertNotEqual(0, failed.returncode)
            self.assertIn('"mismatched": ["lab-linux.tar.gz"]', failed.stdout)

    def test_manifest_binds_archive_and_image_sboms(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            archive = work / "lab-linux.tar.gz"
            archive_sbom = work / "lab-linux.spdx.json"
            image_sbom = work / "lab-container-image.spdx.json"
            for path, data in ((archive, b"archive"), (archive_sbom, b"archive-sbom"), (image_sbom, b"image-sbom")):
                path.write_bytes(data)
            manifest = work / "manifest.json"
            result = subprocess.run([
                "python3", str(ROOT / "scripts/ci/create-release-manifest.py"),
                "--tag", "v1.2.3", "--repository", "acme/labby", "--output", str(manifest),
                "--incus-sha256", "1" * 64, "--mcp-manifest-sha256", "2" * 64,
                "--image", "ghcr.io/acme/labby", "--image-digest", "sha256:" + "1" * 64,
                "--image-sbom", str(image_sbom), str(archive), str(archive_sbom),
            ], text=True, capture_output=True, check=False)
            self.assertEqual(0, result.returncode, result.stderr)
            data = json.loads(manifest.read_text())
            subject = next(row for row in data["subjects"] if row["name"] == archive.name)
            self.assertEqual(archive_sbom.name, subject["sbom"]["name"])
            self.assertEqual("sha256:" + "1" * 64, data["distributions"]["ghcr"]["digest"])
            self.assertEqual(image_sbom.name, data["distributions"]["ghcr"]["sbom"]["name"])

    def test_manifest_rejects_invalid_incus_and_mcp_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); archive = work / "lab.tar.gz"; sbom = work / "lab.spdx.json"
            archive.write_bytes(b"a"); sbom.write_bytes(b"s")
            base = ["python3", str(ROOT / "scripts/ci/create-release-manifest.py"), "--tag", "v1.0.0", "--repository", "a/b", "--incus-sha256", "1" * 64, "--mcp-manifest-sha256", "2" * 64, str(archive), str(sbom)]
            for flag in ("--incus-sha256", "--mcp-manifest-sha256"):
                broken = list(base)
                broken[broken.index(flag) + 1] = "bad"
                self.assertNotEqual(0, subprocess.run(broken, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode)

    def test_reconciler_requires_every_distribution_and_exact_subject(self) -> None:
        expected = {
            "schema": "ai.dinglebear.labby/release-manifest/v1", "tag": "v1.2.3",
            "subjects": [{"name": "a", "sha256": "1" * 64, "sbom": {"name": "a.spdx.json", "sha256": "2" * 64}}],
            "distributions": {name: {"identity": name} for name in ("github", "npm", "ghcr", "incus", "mcp")},
        }
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp); manifest = work / "manifest"; observed = work / "observed"
            manifest.write_text(json.dumps(expected))
            for missing in (None, "npm", "ghcr", "incus", "mcp"):
                current = json.loads(json.dumps(expected))
                current["distributions"].pop(missing, None)
                current["subjects"].append(expected["subjects"][0]["sbom"])
                observed.write_text(json.dumps(current))
                result = subprocess.run(["python3", str(ROOT / "scripts/ci/reconcile-release.py"), "--manifest", str(manifest), "--observed", str(observed)], check=False)
                self.assertEqual(0 if missing is None else 1, result.returncode, missing)

    def test_observer_has_authoritative_remote_probe_for_every_surface(self) -> None:
        observer = (ROOT / "scripts/ci/observe-release.py").read_text()
        for contract in ("gh release", "npm", "imagetools", "incus", "registry.modelcontextprotocol.io/v0.1"):
            self.assertIn(contract, observer)

    def test_upgrade_qualification_consumes_exact_built_candidate(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        job = workflow[workflow.index("  upgrade-qualification:"):workflow.index("  incus-candidate:")]
        self.assertIn("actions/download-artifact@", job)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_BINARY", job)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_SHA256", job)
        self.assertNotIn("LABBY_ALLOW_SOURCE_FALLBACK=1", job)

    def test_n_minus_one_verifies_attested_archive_before_extraction(self) -> None:
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        build = workflow[workflow.index("  build:"):workflow.index("  container:")]
        self.assertIn("actions/attest-build-provenance@", build)
        self.assertIn("subject-path: ${{ matrix.archive }}", build)
        qualification = workflow[workflow.index("  upgrade-qualification:"):workflow.index("  incus-candidate:")]
        verify = qualification.index("verify-release-provenance.sh")
        extract = qualification.index("tar -C candidate")
        self.assertLess(verify, extract)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_ARCHIVE", qualification)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_ARCHIVE_SHA256", qualification)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_BINDING", qualification)

    def test_all_six_n_minus_one_adapters_exist_and_are_executable(self) -> None:
        for name in ("unix", "windows", "macos", "compose", "incus", "host-service"):
            path = ROOT / "scripts/ci/n-minus-one" / name
            self.assertTrue(path.is_file(), name)
            self.assertTrue(os.access(path, os.X_OK), name)
            adapter = path.read_text()
            self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_ARCHIVE", adapter, name)
            self.assertIn("verify-provenance", adapter, name)

    def test_all_six_n_minus_one_adapters_verify_every_durable_class(self) -> None:
        helper = self.text("scripts/ci/n-minus-one-durable-state.py")
        for state_class in ("auth.db", "access.db", "usage.db", "skills/", "artifacts/", "snippets/"):
            self.assertIn(state_class, helper)
        for name in ("unix", "windows", "macos", "compose", "incus", "host-service"):
            self.assertIn("n-minus-one-durable-state.py", self.text(f"scripts/ci/n-minus-one/{name}"), name)

    def test_durable_state_probe_detects_each_missing_class(self) -> None:
        helper = ROOT / "scripts/ci/n-minus-one-durable-state.py"
        with tempfile.TemporaryDirectory() as tmp:
            schemas = {
                "auth.db": "CREATE TABLE registered_clients(client_id TEXT PRIMARY KEY,redirect_uris TEXT NOT NULL,created_at INTEGER NOT NULL)",
                "access.db": "CREATE TABLE access_security_events(event_id TEXT PRIMARY KEY,occurred_at INTEGER NOT NULL,event_kind TEXT NOT NULL,decision TEXT NOT NULL,reason_code TEXT NOT NULL,target_fingerprint BLOB NOT NULL,peer_fingerprint BLOB,metadata_json TEXT NOT NULL)",
                "usage.db": "CREATE TABLE upstream_calls(id INTEGER PRIMARY KEY AUTOINCREMENT,ts_unix INTEGER NOT NULL,upstream_name TEXT NOT NULL,tool_name TEXT NOT NULL,capability TEXT NOT NULL,operation TEXT NOT NULL,subject_scoped INTEGER NOT NULL,actor TEXT NOT NULL,outcome TEXT NOT NULL,elapsed_ms INTEGER NOT NULL,response_bytes INTEGER)",
            }
            for name, schema in schemas.items():
                with sqlite3.connect(Path(tmp) / name) as database:
                    database.execute(schema)
            subprocess.run(["python3", str(helper), "seed", tmp], check=True)
            self.assertEqual(0, subprocess.run(["python3", str(helper), "verify", tmp]).returncode)
            for database, statement in (
                ("auth.db", "DELETE FROM registered_clients"),
                ("access.db", "DELETE FROM access_security_events"),
                ("usage.db", "DELETE FROM upstream_calls"),
            ):
                with sqlite3.connect(Path(tmp) / database) as connection:
                    connection.execute(statement)
                self.assertNotEqual(0, subprocess.run(["python3", str(helper), "verify", tmp], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode, database)
                subprocess.run(["python3", str(helper), "seed", tmp], check=True)
            for relative in ("auth.db", "access.db", "usage.db", "skills/n-minus-one/SKILL.md", "artifacts/n-minus-one/probe.txt", "snippets/n-minus-one/probe.txt"):
                target = Path(tmp) / relative
                saved = target.read_bytes()
                target.unlink()
                self.assertNotEqual(0, subprocess.run(["python3", str(helper), "verify", tmp], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode, relative)
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(saved)

    def test_n_minus_one_adapters_preserve_final_authenticated_proof(self) -> None:
        compose = self.text("scripts/ci/n-minus-one/compose")
        self.assertNotIn("$stage == verify-rollback", compose)
        self.assertIn("$stage == authenticated-action", compose)
        macos = self.text("scripts/ci/n-minus-one/macos")
        seed = macos[macos.index("seed-state)"):macos.index("verify-previous)")]
        self.assertIn("service restart", seed)
        incus = self.text("scripts/ci/n-minus-one/incus")
        self.assertNotIn("target/debug/labby", incus)
        self.assertIn("LABBY_N_MINUS_ONE_CANDIDATE_BINARY", incus)

    def test_post_rollback_requires_restart_and_authenticated_action(self) -> None:
        driver = (ROOT / "scripts/ci/qualify-n-minus-one.sh").read_text()
        self.assertIn("post_rollback_stages=(restart verify_rollback authenticated_action)", driver)

    def test_reconciliation_observation_failure_still_opens_incident(self) -> None:
        workflow = (ROOT / ".github/workflows/release-publish-reminder.yml").read_text()
        job = workflow[workflow.index("  reconcile:"):]
        self.assertIn("id: observe", job)
        self.assertIn("continue-on-error: true", job)
        self.assertIn("observation failed", job)
        self.assertIn("steps.observe.outcome == 'success'", job)

    def test_release_manifest_and_reconciler_cover_checksums_and_attestations(self) -> None:
        workflow = self.text(".github/workflows/release.yml")
        self.assertIn("lab-*.sha256", workflow)
        manifest = self.text("scripts/ci/create-release-manifest.py")
        self.assertIn('"auxiliary"', manifest)
        observer = self.text("scripts/ci/observe-release.py")
        self.assertIn("verify-release-provenance.sh", observer)
        reconciler = self.text("scripts/ci/reconcile-release.py")
        self.assertIn("attestation_errors", reconciler)

    def test_mcp_semantic_observation_can_converge_without_inventing_digest(self) -> None:
        observer = self.text("scripts/ci/observe-release.py")
        self.assertNotIn('"status": "semantic_only"', observer)
        self.assertIn("manifest_sha256", observer)
        reconciler = self.text("scripts/ci/reconcile-release.py")
        self.assertNotIn("semantic_only", reconciler)

    def test_reconciler_runs_immediately_after_release(self) -> None:
        reminder = self.text(".github/workflows/release-publish-reminder.yml")
        self.assertIn('workflows: ["Release", "release-please"]', reminder)

    def test_rollback_rejects_false_success_until_version_absent_and_latest_restored(self) -> None:
        script = (ROOT / "scripts/ci/release-image-rollback.sh").read_text()
        self.assertIn("verify_release_absent", script)
        self.assertIn("ROLLBACK_VERIFY_ATTEMPTS", script)
        self.assertIn("release_tag_absent", script)
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "fake"
            fake.write_text("#!/bin/sh\ncase \" $* \" in *':latest '*) printf '\"sha256:%064d\"\\n' 0; exit 0;; *inspect*) exit 7;; *) exit 0;; esac\n")
            fake.chmod(0o755)
            env = os.environ | {"GH_BIN": str(fake), "DOCKER_BIN": str(fake), "ROLLBACK_VERIFY_ATTEMPTS": "1", "ROLLBACK_VERIFY_DELAY_SECONDS": "0"}
            result = subprocess.run(["bash", str(ROOT / "scripts/ci/release-image-rollback.sh"), "--image", "ghcr.io/a/b", "--tag", "v1", "--previous-latest", "none", "--delete-version-id", ""], env=env, check=False)
            self.assertNotEqual(0, result.returncode, "empty lookup plus opaque inspect failure must not prove absence")


if __name__ == "__main__":
    unittest.main()
