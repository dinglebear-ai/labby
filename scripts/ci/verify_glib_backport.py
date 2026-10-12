#!/usr/bin/env python3
"""Verify the exact GTK3-compatible source correction before advisory audit."""

import hashlib
import json
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[2]
DESKTOP = ROOT / "apps/tauri/src-tauri"
ARCHIVE_SHA = "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5"


def verify(desktop: Path = DESKTOP) -> None:
    receipt = json.loads((desktop / "glib-backport.json").read_text())
    assert receipt["package"] == "glib" and receipt["version"] == "0.18.5"
    assert receipt["registry_archive_sha256"] == ARCHIVE_SHA
    assert receipt["advisory"] == "RUSTSEC-2024-0429"
    assert set(receipt["modified_files"]) == {"src/variant_iter.rs"}
    vendor = desktop / "vendor/glib"
    observed = {str(p.relative_to(vendor)) for p in vendor.rglob("*") if p.is_file()}
    assert observed == set(receipt["upstream_files"]), "vendor file set drift"
    for relative, original_sha in receipt["upstream_files"].items():
        parts = PurePosixPath(relative).parts
        assert parts and not PurePosixPath(relative).is_absolute() and ".." not in parts
        path = vendor / relative
        assert not path.is_symlink(), "vendor symlinks are forbidden"
        data = path.read_bytes()
        expected = receipt["modified_files"].get(relative, original_sha)
        assert hashlib.sha256(data).hexdigest() == expected, f"vendor source drift: {relative}"
        if relative == "src/variant_iter.rs":
            old_pointer = b"let p: *mut libc::c_char = std::ptr::null_mut();"
            fixed_pointer = b"let mut p: *mut libc::c_char = std::ptr::null_mut();"
            assert data.count(fixed_pointer) == 1 and old_pointer not in data
            old_argument = b"                &p,"
            fixed_argument = b"                &mut p,"
            assert data.count(fixed_argument) == 1 and old_argument not in data
            restored = data.replace(fixed_pointer, old_pointer).replace(fixed_argument, old_argument)
            assert hashlib.sha256(restored).hexdigest() == original_sha, "backport must contain only upstream's two-line fix"

    import tomllib

    manifest = tomllib.loads((desktop / "Cargo.toml").read_text())
    assert manifest["patch"]["crates-io"]["glib"] == {"path": "vendor/glib"}
    lock = tomllib.loads((desktop / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] == "glib"]
    assert len(packages) == 1 and packages[0]["version"] == "0.18.5"
    assert "source" not in packages[0] and "checksum" not in packages[0], "desktop must select the patched path source"


if __name__ == "__main__":
    verify()
    print("Verified GLib 0.18.5 registry provenance, exact two-line correction and desktop lock selection.")
