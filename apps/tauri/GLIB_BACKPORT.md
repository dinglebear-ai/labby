# GTK3-compatible GLib security correction

Tauri's GTK3 dependencies require GLib 0.18. The desktop patches that package
with the crates.io 0.18.5 source and the two-line upstream correction
for [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html).
[gtk-rs-core PR 1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343)
makes the output pointer mutable and passes its mutable reference to C.
This fixes the undefined behavior without substituting incompatible GTK APIs.

`src-tauri/glib-backport.json` records the original archive SHA-256, every
packaged file hash and each corrected file hash. The vendored package retains
its upstream license and copyright. `scripts/ci/verify_glib_backport.py`
rejects file drift, checks that undoing exactly those two security lines reproduces
the original iterator file, and verifies that the desktop lock selects this path.
Other locked desktop dependency versions stay unchanged.

Rust 1.97.1 applies the repository's strict warnings policy to this local path
dependency. `src-tauri/glib-compatibility.json` records 31 explicit lifetime
annotations and three redundant type-parenthesis removals requested by that
compiler, plus one versioned Rustdoc URL in the upstream README for Markdown
link validation. These changes preserve the existing lifetimes and behavior.
The verifier pins that correction list and the original archive inventory,
then reverses every listed change and checks the original file hashes.
Warnings remain errors; no vendor-wide lint waiver is used.

Required Linux desktop CI runs that verifier and the small independent
`glib-backport-check` crate in release mode before native desktop tests and
advisory audits of both independent locks. Its tests exercise `next`, `next_back`, `nth`,
`nth_back`, `last`, mixed iteration, empty arrays and overflow boundaries.
They require the real GLib system library; a macOS-only or source-only check
does not replace Linux qualification.

The advisory exception identifies this source-corrected backport because
the version-based database still flags 0.18.5. It does not claim the affected
iterator is unreachable or accept an unfixed vulnerability. Remove the
patch, verifier, helper and exception when the supported Tauri/GTK3 graph
can use an upstream fixed release; regenerate and qualify both locks.
The owning package is Labby Desktop. Backport recorded 2026-10-12.
