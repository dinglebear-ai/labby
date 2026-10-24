"""Reject source drift and accidentally restored vulnerable GTK3 code."""

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from scripts.ci.verify_glib_backport import DESKTOP, verify


class ArchiveSafetyTests(unittest.TestCase):
    def test_archive_paths_and_links_cannot_escape_cache(self):
        import io
        import tarfile
        from scripts.ci.verify_glib_backport import extract_archive
        for name, kind in [("glib-0.18.5/../../escape", tarfile.REGTYPE), ("glib-0.18.5/link", tarfile.SYMTYPE)]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temp:
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = "/etc/passwd"
                    archive.addfile(member)
                with self.assertRaises(AssertionError):
                    extract_archive(buffer.getvalue(), Path(temp), {})
                self.assertEqual(list(Path(temp).iterdir()), [])


    def test_valid_inventory_materializes_exact_bytes(self):
        import hashlib
        import io
        import tarfile
        from scripts.ci.verify_glib_backport import extract_archive
        with tempfile.TemporaryDirectory() as temp:
            buffer = io.BytesIO()
            with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
                member = tarfile.TarInfo("glib-0.18.5/src/lib.rs")
                member.size = 6
                archive.addfile(member, io.BytesIO(b"source"))
            destination = Path(temp)
            extract_archive(buffer.getvalue(), destination, {"src/lib.rs": hashlib.sha256(b"source").hexdigest()})
            self.assertEqual((destination / "src/lib.rs").read_bytes(), b"source")
            with self.assertRaises(AssertionError):
                extract_archive(buffer.getvalue(), destination, {"src/lib.rs": "0" * 64})



class DesktopPreparationWorkflowTests(unittest.TestCase):
    def test_bundle_and_release_lock_entrypoints_prepare_before_cargo_resolution(self):
        import yaml
        root = DESKTOP.parents[2]
        build = yaml.safe_load((root / ".github/workflows/build-desktop.yml").read_text())
        steps = build["jobs"]["build"]["steps"]
        prepare = next(i for i, step in enumerate(steps)
                       if "verify_glib_backport.py --prepare" in step.get("run", ""))
        builds = [i for i, step in enumerate(steps) if "tauri build" in step.get("run", "")]
        self.assertEqual(len(builds), 2)
        self.assertTrue(all(prepare < build for build in builds))
        release = yaml.safe_load((root / ".github/workflows/release-please.yml").read_text())
        commands = "\n".join(step.get("run", "") for step in release["jobs"]["sync-release-version"]["steps"])
        self.assertLess(commands.index("verify_glib_backport.py --prepare"),
                        commands.index("cargo update --manifest-path apps/tauri/src-tauri/Cargo.toml"))


class GlibBackportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.desktop = Path(self.temp.name)
        shutil.copytree(DESKTOP / "target/glib-backport", self.desktop / "target/glib-backport")
        for name in ("glib-backport.json", "glib-compatibility.json", "glib-source-corrections.json", "Cargo.toml", "Cargo.lock"):
            shutil.copyfile(DESKTOP / name, self.desktop / name)

    def test_exact_backport_and_path_lock_are_accepted(self) -> None:
        verify(self.desktop)

    def test_vulnerable_pointer_cannot_be_restored_by_updating_a_receipt(self) -> None:
        import hashlib

        path = self.desktop / "target/glib-backport/glib/src/variant_iter.rs"
        data = path.read_bytes().replace(b"let mut p: *mut libc::c_char", b"let p: *mut libc::c_char").replace(b"                &mut p,", b"                &p,")
        path.write_bytes(data)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["modified_files"]["src/variant_iter.rs"] = hashlib.sha256(data).hexdigest()
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_unrelated_vendor_modification_and_extra_files_are_rejected(self) -> None:
        path = self.desktop / "target/glib-backport/glib/LICENSE"
        original = path.read_bytes()
        path.write_bytes(original + b"drift")
        with self.assertRaises(AssertionError):
            verify(self.desktop)
        path.write_bytes(original)
        (self.desktop / "target/glib-backport/glib/unlisted.rs").write_text("drift")
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_unreviewed_compatibility_edit_cannot_be_accepted_by_a_new_file_hash(self) -> None:
        import hashlib

        path = self.desktop / "target/glib-backport/glib/src/object.rs"
        data = path.read_bytes().replace(b"InterfaceRef<'_, T>", b"InterfaceRef<'static, T>")
        self.assertNotEqual(data, path.read_bytes())
        path.write_bytes(data)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["modified_files"]["src/object.rs"] = hashlib.sha256(data).hexdigest()
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_compatibility_allowlist_and_original_manifest_are_pinned(self) -> None:
        path = self.desktop / "glib-compatibility.json"
        original = path.read_bytes()
        path.write_bytes(original.replace(b"InterfaceRef<'_, T>", b"InterfaceRef<'static, T>"))
        with self.assertRaises(AssertionError):
            verify(self.desktop)
        path.write_bytes(original)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["upstream_files"]["LICENSE"] = "0" * 64
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_collection_guard_cannot_be_removed_by_updating_receipt(self) -> None:
        import hashlib
        path = self.desktop / "target/glib-backport/glib/src/collections/slice.rs"
        data = path.read_bytes().replace(b"        if value.is_empty() {\n            return Self::new();\n        }\n", b"", 1)
        self.assertNotEqual(data, path.read_bytes())
        path.write_bytes(data)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["modified_files"]["src/collections/slice.rs"] = hashlib.sha256(data).hexdigest()
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_spawn_failure_outputs_are_read_only_in_success_branch(self) -> None:
        # GLib leaves PID/FD outputs uninitialized when spawning fails.
        source = (self.desktop / "target/glib-backport/glib/src/manual_functions.rs").read_text()
        for name in ("spawn_async_with_fds", "spawn_async_with_pipes"):
            function = source.split("pub fn " + name + "<", 1)[1].split("\n}\n", 1)[0]
            self.assertNotIn("assume_init()", function.split("if error.is_null() {", 1)[0], name)

    def test_builder_clone_length_fix_cannot_be_removed_by_updating_receipt(self) -> None:
        import hashlib
        path = self.desktop / "target/glib-backport/glib/src/gstring_builder.rs"
        data = path.read_bytes().replace(b"len: (*src).len,", b"len: 0,", 1)
        self.assertNotEqual(data, path.read_bytes())
        path.write_bytes(data)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["modified_files"]["src/gstring_builder.rs"] = hashlib.sha256(data).hexdigest()
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_each_corrected_source_file_requires_its_pinned_edit(self) -> None:
        import hashlib
        corrections = json.loads((self.desktop / "glib-source-corrections.json").read_text())["changes"]
        receipt_path = self.desktop / "glib-backport.json"
        original_receipt = receipt_path.read_text()
        # Later edits can refine an earlier replacement. Reverse each file's
        # recipe in dependency order so every exact replacement is exercised.
        reversed_files = {}
        for index, correction in reversed(list(enumerate(corrections))):
            relative = correction["file"]
            with self.subTest(file=relative, correction=index):
                path = self.desktop / "target/glib-backport/glib" / relative
                original = path.read_bytes()
                current = reversed_files.get(relative, original)
                reverted = current.replace(correction["new"].encode(), correction["old"].encode(), 1)
                self.assertNotEqual(reverted, current)
                reversed_files[relative] = reverted
                path.write_bytes(reverted)
                receipt = json.loads(original_receipt)
                receipt["modified_files"][relative] = hashlib.sha256(reverted).hexdigest()
                receipt_path.write_text(json.dumps(receipt))
                with self.assertRaises(AssertionError):
                    verify(self.desktop)
                path.write_bytes(original)
                receipt_path.write_text(original_receipt)

    def test_registry_lock_cannot_bypass_the_path_correction(self) -> None:
        path = self.desktop / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "glib"\nversion = "0.18.5"', 'name = "glib"\nversion = "0.18.5"\nsource = "registry+https://github.com/rust-lang/crates.io-index"'))
        with self.assertRaises(AssertionError):
            verify(self.desktop)


if __name__ == "__main__":
    unittest.main()
