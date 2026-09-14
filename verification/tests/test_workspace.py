"""Executable workspace contracts; these are not model or product E2E tests."""

import json
from pathlib import Path
import subprocess
import tomllib
from typing import Any
import unittest


WORKSPACE = Path(__file__).resolve().parents[1]
ROOT = WORKSPACE.parent


def read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as source:
        return tomllib.load(source)


def metadata(path: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--manifest-path", str(path / "Cargo.toml"),
         "--format-version", "1", "--no-deps", "--offline", "--locked"],
        cwd=ROOT, check=True, capture_output=True, text=True, timeout=30,
    )
    return json.loads(result.stdout)


def assert_no_toolkit_dependencies(packages: list[dict[str, Any]]) -> None:
    for package in packages:
        for dependency in package["dependencies"]:
            path = dependency.get("path")
            # C1's public-process adapter is test-only. Neither production nor
            # build dependencies may reach the model or verification toolkit.
            if (package["name"] == "labby" and dependency.get("kind") == "dev"
                    and path and (
                        (dependency["name"] == "labby-model"
                         and Path(path).resolve() == ROOT / "crates/labby-model")
                        or (dependency["name"] == "verify-core"
                            and Path(path).resolve() == WORKSPACE / "crates/verify-core"))):
                continue
            if dependency["name"] == "labby-model" or (
                path and Path(path).resolve() == ROOT / "crates/labby-model"
            ):
                raise AssertionError(f"product dependency on model: {package['name']}")
            if dependency["name"].startswith("verify-") or (
                path and Path(path).resolve().is_relative_to(WORKSPACE)
            ):
                # The sole M3 exception is the dev-facing model's two pure leaves.
                # Check identity AND resolved path; renamed paths cannot hide a backend.
                if (package["name"] == "labby-model"
                        and dependency["name"] in {"verify-core", "verify-scenario"}
                        and path
                        and Path(path).resolve() == WORKSPACE / "crates" / dependency["name"]):
                    continue
                raise AssertionError(f"product dependency on toolkit: {package['name']}")


def assert_pure_leaf_dependencies(packages: list[dict[str, Any]]) -> None:
    """Seal both direct edges in the pure toolkit closure, including renamed paths."""
    expected = {
        "verify-core": {"serde", "serde_json", "thiserror", "toml", "schemars"},
        "verify-scenario": {"verify-core", "serde", "serde_json", "thiserror", "schemars", "blake3"},
    }
    for name, allowed in expected.items():
        package = next(p for p in packages if p["name"] == name)
        dependencies = [d for d in package["dependencies"] if d.get("kind") != "dev"]
        if {d["name"] for d in dependencies} != allowed:
            raise AssertionError(f"impure leaf dependency: {name}")
        for dependency in dependencies:
            path = dependency.get("path")
            expected_path = WORKSPACE / "crates/verify-core" if dependency["name"] == "verify-core" else None
            if ((expected_path is None and path)
                    or (expected_path is not None and (not path or Path(path).resolve() != expected_path))):
                raise AssertionError(f"unexpected leaf dependency path: {name}")


class WorkspaceContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.product = metadata(ROOT)
        cls.toolkit = metadata(WORKSPACE)

    def test_distinct_workspace_roots_and_members(self) -> None:
        self.assertEqual(Path(self.product["workspace_root"]), ROOT)
        self.assertEqual(Path(self.toolkit["workspace_root"]), WORKSPACE)
        self.assertTrue(self.toolkit["workspace_members"])
        self.assertTrue(set(self.product["workspace_members"]).isdisjoint(
            self.toolkit["workspace_members"]))
        self.assertIn("verification", read_toml(ROOT / "Cargo.toml")["workspace"]["exclude"])

    def test_product_does_not_depend_on_toolkit(self) -> None:
        assert_no_toolkit_dependencies(self.product["packages"])

    def test_guard_rejects_named_toolkit_dependency(self) -> None:
        with self.assertRaisesRegex(AssertionError, "product dependency"):
            assert_no_toolkit_dependencies([
                {"name": "fixture-product", "dependencies": [{"name": "verify-core"}]}])

    def test_guard_rejects_renamed_path_dependency(self) -> None:
        with self.assertRaisesRegex(AssertionError, "product dependency"):
            assert_no_toolkit_dependencies([{"name": "fixture-product", "dependencies": [
                {"name": "innocent-name", "path": str(WORKSPACE / "crates/verify-core")}
            ]}])

    def test_only_pure_model_leaf_exception_is_allowed(self) -> None:
        for name in ("verify-core", "verify-scenario"):
            assert_no_toolkit_dependencies([{"name": "labby-model", "dependencies": [
                {"name": name, "path": str(WORKSPACE / "crates" / name)}]}])
        for dependency in [
            {"name": "verify-runner", "path": str(WORKSPACE / "crates/verify-runner"), "kind": "dev"},
            {"name": "verify-core", "path": str(WORKSPACE / "crates/verify-runner")},
            {"name": "alias", "path": str(WORKSPACE / "hosts/labby")},
        ]:
            with self.assertRaises(AssertionError):
                assert_no_toolkit_dependencies([{"name": "labby-model", "dependencies": [dependency]}])

    def test_guard_rejects_product_backedges_to_model(self) -> None:
        for dependency in [{"name": "labby-model"},
                           {"name": "renamed", "path": str(ROOT / "crates/labby-model")}]:
            with self.assertRaisesRegex(AssertionError, "product dependency on model"):
                assert_no_toolkit_dependencies([{"name": "labby", "dependencies": [dependency]}])

    def test_conformance_exception_is_exact_and_dev_only(self) -> None:
        for name, path in [
            ("labby-model", ROOT / "crates/labby-model"),
            ("verify-core", WORKSPACE / "crates/verify-core"),
        ]:
            dependency = {"name": name, "path": str(path), "kind": "dev"}
            assert_no_toolkit_dependencies([{"name": "labby", "dependencies": [dependency]}])
            for changed in [dict(dependency, kind=None), dict(dependency, kind="build"),
                            dict(dependency, name="renamed"),
                            dict(dependency, path=str(WORKSPACE / "crates/verify-runner"))]:
                with self.assertRaises(AssertionError):
                    assert_no_toolkit_dependencies([{"name": "labby", "dependencies": [changed]}])
            with self.assertRaises(AssertionError):
                assert_no_toolkit_dependencies([{"name": "labby-gateway", "dependencies": [dependency]}])

    def test_model_has_only_pure_dependencies(self) -> None:
        model = next(p for p in self.product["packages"] if p["name"] == "labby-model")
        self.assertTrue({d["name"] for d in model["dependencies"]} <=
                        {"verify-core", "verify-scenario", "serde", "serde_json", "labby-primitives"})
        self.assertEqual(len(self.product["workspace_members"]), 13)

    def test_core_dependencies_remain_pure_and_backends_are_not_activated(self) -> None:
        assert_pure_leaf_dependencies(self.toolkit["packages"])
        core = next(p for p in self.toolkit["packages"] if p["name"] == "verify-core")
        normal = {d["name"] for d in core["dependencies"] if d["kind"] is None}
        self.assertEqual(normal, {"serde", "serde_json", "thiserror", "toml", "schemars"})
        self.assertEqual({d["name"] for d in core["dependencies"] if d["kind"] == "dev"},
                         {"jsonschema"})
        self.assertEqual(core["publish"], [])

    def test_leaf_guard_rejects_adapter_hidden_beneath_scenario(self) -> None:
        from copy import deepcopy

        for name, path in [("unexpected-adapter", None),
                           ("serde_json", str(WORKSPACE / "hosts/labby"))]:
            packages = deepcopy(self.toolkit["packages"])
            scenario = next(p for p in packages if p["name"] == "verify-scenario")
            scenario["dependencies"].append({"name": name, "kind": None, "path": path})
            with self.assertRaises(AssertionError):
                assert_pure_leaf_dependencies(packages)

    def test_msrv_matches_product_and_pinned_toolchain(self) -> None:
        version = read_toml(ROOT / "Cargo.toml")["workspace"]["package"]["rust-version"]
        self.assertEqual(version, read_toml(ROOT / "rust-toolchain.toml")["toolchain"]["channel"])
        for package in self.toolkit["packages"]:
            self.assertEqual(package["rust_version"], version)

    def test_dependencies_are_exact_pins_and_schema_has_no_remote_defaults(self) -> None:
        dependencies = read_toml(WORKSPACE / "Cargo.toml")["workspace"]["dependencies"]
        for name, declaration in dependencies.items():
            version = declaration if isinstance(declaration, str) else declaration["version"]
            with self.subTest(name=name):
                self.assertRegex(version, r"^=\d+\.\d+\.\d+$")
        self.assertIs(dependencies["jsonschema"]["default-features"], False)

    def test_external_tools_are_explicitly_qualified_or_disabled(self) -> None:
        tools = read_toml(WORKSPACE / "toolchain.toml")
        self.assertEqual(tools["schema"], 1)
        self.assertIs(tools["kani"]["enabled"], True)
        self.assertEqual(tools["kani"]["version"], "0.67.0")
        self.assertRegex(tools["kani"]["asset_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(tools["kani"]["driver_env"], "LABBY_KANI_DRIVER")
        for name in ("alloy", "tlc"):
            with self.subTest(name=name):
                self.assertIs(tools[name]["enabled"], True)
                self.assertRegex(tools[name]["jar_sha256"], r"^[0-9a-f]{64}$")
                self.assertTrue(tools[name]["source"].startswith("https://github.com/"))
        for name in ("apalache",):
            with self.subTest(name=name):
                self.assertIs(tools[name]["enabled"], False)
                self.assertTrue(tools[name]["reason"])
                self.assertRegex(tools[name]["image"], r"@sha256:[0-9a-f]{64}$")

    def test_all_member_dependency_requirements_are_exact(self) -> None:
        for package in self.toolkit["packages"]:
            for dependency in package["dependencies"]:
                with self.subTest(package=package["name"], dependency=dependency["name"]):
                    self.assertRegex(dependency["req"], r"^=\d+\.\d+\.\d+$")

    def test_lockfile_keeps_opted_in_backend_out_of_product(self) -> None:
        lock = read_toml(WORKSPACE / "Cargo.lock")
        names = {p["name"] for p in lock["package"]}
        self.assertIn("verify-core", names)
        self.assertTrue({"verify-stateright", "stateright", "verify-report", "insta",
                         "verify-loom", "loom", "shuttle", "verify-kani",
                         "verify-tla", "verify-alloy"} <= names)
        self.assertTrue(names.isdisjoint({"kani-verifier",
                                         "tokio", "reqwest", "rmcp", "aws-lc-sys"}))
        product_lock = read_toml(ROOT / "Cargo.lock")
        product_names = {p["name"] for p in product_lock["package"]}
        self.assertTrue({"verify-core", "verify-scenario", "labby-model"} <= product_names)
        self.assertTrue(product_names.isdisjoint({"verify-runner", "labby-verify", "verify-stateright", "verify-report", "stateright", "verify-loom", "loom", "shuttle", "verify-kani", "verify-tla", "verify-alloy"}))


if __name__ == "__main__":
    unittest.main()
