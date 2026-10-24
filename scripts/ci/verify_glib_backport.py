#!/usr/bin/env python3
"""Verify the exact GTK3-compatible source correction before advisory audit."""

import hashlib
import json
from pathlib import Path, PurePosixPath
import io
import gzip
import time
import tarfile
import tempfile
import urllib.request
import os


ROOT = Path(__file__).resolve().parents[2]
DESKTOP = ROOT / "apps/tauri/src-tauri"
ARCHIVE_SHA = "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5"
UPSTREAM_MANIFEST_SHA = "d59db0cd23975279aa057124dd43c47d26ba742df34e9dbfba0e72099d71e217"
COMPATIBILITY_SHA = "2138b16f6895d1940e5cfda557f80e14dc6332c5e1bd538802f4c82f7c99980e"

SOURCE_CORRECTIONS_SHA = "34594b0465a0795392668432e799d0f7e3e7edafea21e6848d72bc9de68d1644"


def require(condition: bool, message: str = "GLib provenance validation failed") -> None:
    """Keep trust checks active even under python -O."""
    if not condition:
        raise AssertionError(message)


def verify(desktop: Path = DESKTOP, cache: Path | None = None) -> None:
    receipt = json.loads((desktop / "glib-backport.json").read_text())
    require(receipt["package"] == "glib" and receipt["version"] == "0.18.5")
    require(receipt["registry_archive_sha256"] == ARCHIVE_SHA)
    require(receipt["advisory"] == "RUSTSEC-2024-0429")
    original_manifest = json.dumps(receipt["upstream_files"], sort_keys=True, separators=(",", ":")).encode()
    require(hashlib.sha256(original_manifest).hexdigest() == UPSTREAM_MANIFEST_SHA, "original archive inventory drift")
    compatibility_data = (desktop / "glib-compatibility.json").read_bytes()
    require(hashlib.sha256(compatibility_data).hexdigest() == COMPATIBILITY_SHA, "reviewed compatibility corrections drift")
    compatibility = json.loads(compatibility_data)
    require(compatibility["schema_version"] == 1)
    require(compatibility["package"] == "glib" and compatibility["version"] == "0.18.5")
    changes = compatibility["changes"]
    require(len(changes) == 35)
    compatibility_files = {change["file"] for change in changes}
    require("src/variant_iter.rs" not in compatibility_files)
    source_data = (desktop / "glib-source-corrections.json").read_bytes()
    require(hashlib.sha256(source_data).hexdigest() == SOURCE_CORRECTIONS_SHA)
    source_changes = json.loads(source_data)["changes"]
    source_files = {change["file"] for change in source_changes}
    require(set(receipt["modified_files"]) == {"src/variant_iter.rs"} | compatibility_files | source_files)
    vendor = cache if cache is not None else desktop / "target/glib-backport/glib"
    require(not vendor.is_symlink(), "cache symlinks are forbidden")
    observed = {p.relative_to(vendor).as_posix() for p in vendor.rglob("*") if p.is_file()}
    require(observed == set(receipt["upstream_files"]), "vendor file set drift")
    for relative, original_sha in receipt["upstream_files"].items():
        parts = PurePosixPath(relative).parts
        require(parts and not PurePosixPath(relative).is_absolute() and ".." not in parts)
        path = vendor / relative
        require(not path.is_symlink(), "vendor symlinks are forbidden")
        data = path.read_bytes()
        expected = receipt["modified_files"].get(relative, original_sha)
        require(hashlib.sha256(data).hexdigest() == expected, f"vendor source drift: {relative}")
        restored = data
        if relative in source_files:
            text = restored.decode()
            for change in reversed(source_changes):
                if change["file"] == relative:
                    require(text.count(change["new"]) == 1)
                    text = text.replace(change["new"], change["old"])
            restored = text.encode()
        if relative in compatibility_files:
            lines = restored.decode().splitlines(keepends=True)
            seen = set()
            for change in changes:
                if change["file"] != relative:
                    continue
                position = change["edit_line"] - 1
                require(position not in seen and 0 <= position < len(lines))
                seen.add(position)
                require(lines[position] == change["new"] + "\n", f"compatibility correction drift: {relative}:{position + 1}")
                lines[position] = change["old"] + "\n"
            restored = "".join(lines).encode()

        if relative == "src/variant_iter.rs":
            old_pointer = b"let p: *mut libc::c_char = std::ptr::null_mut();"
            fixed_pointer = b"let mut p: *mut libc::c_char = std::ptr::null_mut();"
            require(restored.count(fixed_pointer) == 1 and old_pointer not in restored)
            old_argument = b"                &p,"
            fixed_argument = b"                &mut p,"
            require(restored.count(fixed_argument) == 1 and old_argument not in restored)
            restored = restored.replace(fixed_pointer, old_pointer).replace(fixed_argument, old_argument)
        require(hashlib.sha256(restored).hexdigest() == original_sha, f"unreviewed source changes: {relative}")

    import tomllib

    manifest = tomllib.loads((desktop / "Cargo.toml").read_text())
    require(manifest["patch"]["crates-io"]["glib"] == {"path": "target/glib-backport/glib"})
    lock = tomllib.loads((desktop / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] == "glib"]
    require(len(packages) == 1 and packages[0]["version"] == "0.18.5")
    require("source" not in packages[0] and "checksum" not in packages[0], "desktop must select the patched path source")


def extract_archive(data: bytes, destination: Path, inventory: dict) -> None:
    """Validate the complete archive before writing any member."""
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        unpacked = compressed.read(30_000_001)
    require(len(unpacked) <= 30_000_000, "expanded archive exceeds size budget")
    with tarfile.open(fileobj=io.BytesIO(unpacked), mode="r:") as archive:
        members = archive.getmembers()
        require(len(members) <= 200 and sum(m.size for m in members) <= 30_000_000)
        selected = {}
        for member in members:
            parts = PurePosixPath(member.name).parts
            require(parts and parts[0] == "glib-0.18.5" and ".." not in parts)
            require(member.isfile(), "archive links and special files are forbidden")
            relative = str(PurePosixPath(*parts[1:]))
            require(relative in inventory and relative not in selected)
            content = archive.extractfile(member).read()
            require(hashlib.sha256(content).hexdigest() == inventory[relative])
            selected[relative] = content
        require(set(selected) == set(inventory))
        for relative, content in selected.items():
            path = destination / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)


def prepare(desktop: Path = DESKTOP) -> None:
    """Materialize a verified backport in this checkout's ignored build cache."""
    destination = desktop / "target/glib-backport/glib"
    require(not (desktop / "target").is_symlink() and not destination.parent.is_symlink())
    if destination.exists():
        verify(desktop)
        return
    receipt = json.loads((desktop / "glib-backport.json").read_text())
    require(receipt["registry_archive_sha256"] == ARCHIVE_SHA)
    url = "https://static.crates.io/crates/glib/glib-0.18.5.crate"
    with urllib.request.urlopen(url, timeout=30) as response:
        deadline = time.monotonic() + 30
        chunks = []
        size = 0
        while True:
            require(time.monotonic() < deadline, "archive download exceeded 30-second read budget")
            chunk = response.read1(min(65536, 4_000_001 - size))
            if not chunk:
                break
            chunks.append(chunk)
            size += len(chunk)
            require(size <= 4_000_000, "archive exceeds download size budget")
        data = b"".join(chunks)
    require(len(data) <= 4_000_000 and hashlib.sha256(data).hexdigest() == ARCHIVE_SHA)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="prepare-", dir=destination.parent) as temp:
        staged = Path(temp) / "glib"
        staged.mkdir()
        extract_archive(data, staged, receipt["upstream_files"])
        compatibility_data = (desktop / "glib-compatibility.json").read_bytes()
        require(hashlib.sha256(compatibility_data).hexdigest() == COMPATIBILITY_SHA)
        for change in json.loads(compatibility_data)["changes"]:
            path = staged / change["file"]
            lines = path.read_text().splitlines(keepends=True)
            index = change["edit_line"] - 1
            require(lines[index] == change["old"] + "\n")
            lines[index] = change["new"] + "\n"
            path.write_text("".join(lines), newline="")
        path = staged / "src/variant_iter.rs"
        text = path.read_text()
        for old, new in [("let p: *mut libc::c_char", "let mut p: *mut libc::c_char"), ("                &p,", "                &mut p,")]:
            require(text.count(old) == 1)
            text = text.replace(old, new)
        path.write_text(text, newline="")
        corrections = (desktop / "glib-source-corrections.json").read_bytes()
        require(hashlib.sha256(corrections).hexdigest() == SOURCE_CORRECTIONS_SHA)
        for change in json.loads(corrections)["changes"]:
            path = staged / change["file"]
            text = path.read_text()
            require(text.count(change["old"]) == 1)
            path.write_text(text.replace(change["old"], change["new"]), newline="")
        # Check every patched hash before publishing the cache atomically.
        for relative, original in receipt["upstream_files"].items():
            require(hashlib.sha256((staged / relative).read_bytes()).hexdigest() == receipt["modified_files"].get(relative, original))
        verify(desktop, staged)
        try:
            os.rename(staged, destination)
        except OSError:
            if not destination.exists():
                raise
            # Another preparer may have published the same immutable cache.
            # Verify its complete bytes; never replace or repair a winner.
            verify(desktop)
    verify(desktop)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare", action="store_true", help="fetch and materialize the pinned backport in ignored target storage")
    if parser.parse_args().prepare:
        prepare()
    verify()
    print("Verified off-tree GLib 0.18.5 provenance, security and reviewed source corrections, compatibility edits, and desktop lock selection.")
