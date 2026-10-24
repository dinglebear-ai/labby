# GTK3-compatible GLib security correction

Tauri's GTK3 graph requires GLib 0.18. The latest compatible registry release,
0.18.5, still needs the two-line [RUSTSEC-2024-0429 correction](https://github.com/gtk-rs/gtk-rs-core/pull/1343).
The upstream 0.18 branch at `42b9caf98e03ded086362d9653ca58fe94dc8658`
also retains the affected immutable output pointer. A newer incompatible GLib
release cannot replace this GTK3 dependency independently.

No GLib Rust source is committed. Run `pnpm prepare:glib` from `apps/tauri`
before direct Cargo commands. `pnpm dev` and `pnpm build` do this automatically.
The preparer downloads the exact crates.io 0.18.5 archive, requires SHA-256
`233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`,
validates every packaged file, and materializes the corrected source beneath
`src-tauri/target/glib-backport/glib`, an ignored, checkout-local build cache.
The download has a 30-second connect/socket timeout and a 30-second read-loop
budget; the last blocked read can extend that phase by up to another 30 seconds.
Compressed bytes are capped at 4 MB, expanded bytes at 30 MB, and membership
at 200 regular files. All trust checks remain active under `python -O`.
Concurrent preparers verify the winning atomic cache instead of replacing it.
An absent cache fails Cargo resolution rather than selecting vulnerable registry
code. An existing altered cache fails verification; preparation never silently
repairs it. Remove only this task-owned cache explicitly when rebuilding it.

`src-tauri/glib-backport.json` records archive and corrected file hashes.
`glib-compatibility.json` pins 31 lifetime annotations, three redundant
parenthesis removals, and one versioned Rustdoc link required by Rust 1.97.1.
`glib-source-corrections.json` pins three empty-input guards: safe conversions
between Slice and PtrSlice preserve valid empty ownership, and empty container
conversion avoids copying through a null pointer. It also moves PID/file-descriptor
output reads in both native spawn wrappers inside their success branches, because
GLib leaves those outputs uninitialized when it returns an error. Other edits
preserve GStringBuilder clone length, correct Date parsing and subtraction
validity checks, and preserve List/SList node and item ownership during container
export, import, and retain. Zero-capacity channel `try_send` returns `Full`
without waiting for main-context dispatch. Variant data constructors and consuming
fixed-array conversions require `Send + Sync + 'static` owners; fixed arrays
retain their array wire type. URI segment decoding takes a checked byte offset
into one input allocation and requires a UTF-8 boundary. URI string decoders
reject invalid decoded UTF-8 before constructing a GString. Inline boxed arrays allocate for every copied item.
Translation offsets must identify a UTF-8 boundary in the input. Local task
results can cross threads only when `Send`; aborted handles report cancellation
and task finalization releases retained senders. Log fields borrow their value
bytes for the field lifetime. Flag names combine with bitwise union, string
vectors preserve their null terminators and compare lengths, and spawn wrappers
release setup captures in the parent process after returning. Owning GStringPtr
transfers suppress the original destructor. Foreign GString byte conversion
preserves the full text and trailing null where required, and string-vector
GValue conversion takes owned string copies. The verifier reverses each
correction and requires the original archive hash. It rejects extra files,
symlinks, unrelated changes, and registry lock substitution. No warning or
advisory suppression was added for the collection fixes.

CI prepares the cache explicitly before the verifier, native helper, desktop
checks and advisory audits. The independent `glib-backport-check` crate uses
real native GLib in debug and optimized builds. Tests cover iterator direction,
empty and overflow cases, fresh/reserved/cleared empty collections, null
terminators, nonempty object ownership through weak references, date error
preservation, list container ownership, and bounded nonblocking channel behavior.
Compile-fail doctests reject borrowed Variant backing storage, borrowed fixed
arrays, and non-thread-safe backing owners. These are
regressions, not a proof of whole-crate memory safety or desktop UI acceptance.

The existing advisory exception identifies source-corrected GLib 0.18.5; it does
not accept unfixed iterator behavior. Remove this preparer, correction manifests,
helper, and exception when the supported Tauri/GTK3 graph accepts a fixed upstream
release, then regenerate and qualify both independent locks.
