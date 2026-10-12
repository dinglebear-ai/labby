"""Reject source drift and accidentally restored vulnerable GTK3 code."""

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from scripts.ci.verify_glib_backport import DESKTOP, verify


class GlibBackportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.desktop = Path(self.temp.name)
        shutil.copytree(DESKTOP / "vendor", self.desktop / "vendor")
        for name in ("glib-backport.json", "Cargo.toml", "Cargo.lock"):
            shutil.copyfile(DESKTOP / name, self.desktop / name)

    def test_exact_backport_and_path_lock_are_accepted(self) -> None:
        verify(self.desktop)

    def test_vulnerable_pointer_cannot_be_restored_by_updating_a_receipt(self) -> None:
        import hashlib

        path = self.desktop / "vendor/glib/src/variant_iter.rs"
        data = path.read_bytes().replace(b"let mut p: *mut libc::c_char", b"let p: *mut libc::c_char").replace(b"                &mut p,", b"                &p,")
        path.write_bytes(data)
        receipt_path = self.desktop / "glib-backport.json"
        receipt = json.loads(receipt_path.read_text())
        receipt["modified_files"]["src/variant_iter.rs"] = hashlib.sha256(data).hexdigest()
        receipt_path.write_text(json.dumps(receipt))
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_unrelated_vendor_modification_and_extra_files_are_rejected(self) -> None:
        path = self.desktop / "vendor/glib/LICENSE"
        original = path.read_bytes()
        path.write_bytes(original + b"drift")
        with self.assertRaises(AssertionError):
            verify(self.desktop)
        path.write_bytes(original)
        (self.desktop / "vendor/glib/unlisted.rs").write_text("drift")
        with self.assertRaises(AssertionError):
            verify(self.desktop)

    def test_registry_lock_cannot_bypass_the_path_correction(self) -> None:
        path = self.desktop / "Cargo.lock"
        path.write_text(path.read_text().replace('name = "glib"\nversion = "0.18.5"', 'name = "glib"\nversion = "0.18.5"\nsource = "registry+https://github.com/rust-lang/crates.io-index"'))
        with self.assertRaises(AssertionError):
            verify(self.desktop)


if __name__ == "__main__":
    unittest.main()
