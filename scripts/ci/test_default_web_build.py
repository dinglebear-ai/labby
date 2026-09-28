#!/usr/bin/env python3
"""Source builds must produce usable web assets before compiling Rust."""
from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DefaultWebBuildContractTests(unittest.TestCase):
    def test_product_entrypoints_build_assets_first(self) -> None:
        justfile = (ROOT / "Justfile").read_text()
        for recipe in ("build", "build-release", "host-sync", "host-service-install", "run", "chat-local"):
            with self.subTest(recipe=recipe):
                self.assertRegex(justfile, rf"(?m)^{re.escape(recipe)}(?: \*ARGS)?: web-build$")

    def test_build_inputs_run_policy_tests_and_the_real_frontend_build(self) -> None:
        spec = importlib.util.spec_from_file_location("changed_paths", ROOT / "scripts/ci/changed_paths.py")
        self.assertIsNotNone(spec)
        self.assertIsNotNone(spec.loader)
        classifier = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(classifier)
        for path in ("Justfile", "scripts/build-web.sh", "scripts/ci/test_default_web_build.py"):
            with self.subTest(path=path):
                gates = classifier.classify("pull_request", [path])
                self.assertTrue(gates["workflow"])
                self.assertTrue(gates["web"])

    def test_installers_inherit_the_asset_build(self) -> None:
        justfile = (ROOT / "Justfile").read_text()
        self.assertRegex(justfile, r"(?m)^install: build-release$")
        self.assertRegex(justfile, r"(?m)^macos-service-install: build$")

    def test_web_recipe_uses_the_shared_builder(self) -> None:
        self.assertIn("web-build:\n    bash scripts/build-web.sh\n", (ROOT / "Justfile").read_text())

    def test_rust_only_checks_do_not_require_a_web_build(self) -> None:
        justfile = (ROOT / "Justfile").read_text()
        for recipe in ("check", "test", "lint"):
            with self.subTest(recipe=recipe):
                header = re.search(rf"(?m)^{recipe}:(.*)$", justfile)
                self.assertIsNotNone(header)
                self.assertNotIn("web-build", header.group(1))


class WebBuildScriptTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="labby web build ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.app = self.root / "apps" / "gateway-admin"
        self.app.mkdir(parents=True)
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/build-web.sh", scripts / "build-web.sh")
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.log = self.root / "calls.jsonl"
        pnpm = self.tools / "pnpm"
        pnpm.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ["TEST_CALLS"], "a") as log:
    log.write(json.dumps({"args": args, "cwd": os.getcwd()}) + "\\n")
if os.environ.get("TEST_FAIL") == args[0]:
    sys.exit(42)
if args[0] == "build":
    mode = os.environ.get("TEST_EXPORT", "healthy")
    if mode == "missing":
        sys.exit(0)
    out = pathlib.Path("out")
    out.mkdir(exist_ok=True)
    (out / "index.html").write_text("" if mode == "empty-index" else "<html>Labby</html>")
    if mode == "missing-static":
        sys.exit(0)
    chunks = out / "_next/static/chunks"
    chunks.mkdir(parents=True, exist_ok=True)
    (chunks / "app.js").write_text("" if mode == "empty-js" else "console.log(1)")
''')
        pnpm.chmod(0o755)

    def run_build(self, **extra: str) -> subprocess.CompletedProcess[str]:
        env = {
            **os.environ,
            "PATH": f"{self.tools}{os.pathsep}{os.environ['PATH']}",
            "TEST_CALLS": str(self.log),
            **extra,
        }
        return subprocess.run(
            ["bash", str(self.root / "scripts/build-web.sh")],
            cwd=self.tools, env=env, text=True, capture_output=True,
            timeout=15, check=False,
        )

    def calls(self) -> list[dict]:
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_clean_checkout_installs_locked_build_dependencies_before_export(self) -> None:
        result = self.run_build(NODE_ENV="production")
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        self.assertEqual(
            [["install", "--frozen-lockfile", "--prod=false"], ["build"]],
            [call["args"] for call in self.calls()],
        )
        self.assertTrue(all(Path(call["cwd"]).resolve() == self.app.resolve() for call in self.calls()))
        self.assertTrue((self.app / "out/index.html").is_file())

    def test_install_failure_stops_before_build(self) -> None:
        result = self.run_build(TEST_FAIL="install")
        self.assertEqual(42, result.returncode, result.stderr)
        self.assertEqual(["install"], [call["args"][0] for call in self.calls()])

    def test_failed_build_is_not_masked_by_an_old_export(self) -> None:
        self.assertEqual(0, self.run_build().returncode)
        result = self.run_build(TEST_FAIL="build")
        self.assertEqual(42, result.returncode, result.stderr)

    def test_missing_export_is_rejected(self) -> None:
        result = self.run_build(TEST_EXPORT="missing")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("index.html", result.stderr)

    def test_empty_index_is_rejected(self) -> None:
        result = self.run_build(TEST_EXPORT="empty-index")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("index.html", result.stderr)

    def test_missing_javascript_is_rejected(self) -> None:
        result = self.run_build(TEST_EXPORT="missing-static")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("JavaScript", result.stderr)

    def test_empty_javascript_is_rejected(self) -> None:
        result = self.run_build(TEST_EXPORT="empty-js")
        self.assertNotEqual(0, result.returncode)
        self.assertIn("JavaScript", result.stderr)


if __name__ == "__main__":
    unittest.main()
