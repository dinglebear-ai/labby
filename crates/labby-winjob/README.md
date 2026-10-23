# labby-winjob

`labby-winjob` is Labby's sanctioned unsafe Windows FFI boundary. It exposes
safe process-containment and verified-filesystem APIs while encapsulating raw
`windows-sys` calls. Product, auth, gateway, and Code Mode consumers retain their
own policy and lifecycle coordination. The [manifest](Cargo.toml) records the
explicit unsafe-lint exception and Windows dependencies.

All public implementation is behind `cfg(windows)`. On other targets this crate
is an empty library; a successful Linux build/test does not exercise Job Objects,
Windows sharing rules, DACLs, or file identity. This workspace library has
`publish = false` and no declared Cargo features.

## Job Object ownership and cleanup

[`JobObject::assign(pid)`](src/lib.rs) opens an existing process, creates a job,
enables `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and assigns the process to it.
The returned `JobObject` exclusively owns an opaque, non-cloneable job handle.
The temporary process handle closes inside the function; callers do not receive
or manage raw Win32 handles.

Closing the last job handle causes Windows to terminate assigned processes and
their descendants created after assignment. `close(self)` consumes the owner
and returns a `WinJobError` if explicit closure fails. `Drop` performs cleanup
and logs failures without panicking. Retain the owner for the full lifetime of
the process tree; dropping it at the end of a connection setup helper kills
the tree rather than transferring containment to the caller.

The implementation assigns **after spawn**. Descendants created before
assignment are not retroactively captured, so this API has an accepted
spawn-to-assignment race. Callers must attach promptly, preserve cleanup on
assignment failure, and avoid claiming race-free containment. The crate does
not spawn or suspend the child, wait for its exit, or implement a restart policy.

This Windows-only adapter returns the guard to the owner of an already-spawned
child, using the same public assignment API as the runtime guard:

```rust
#[cfg(windows)]
fn contain_child(
    child: &std::process::Child,
) -> Result<labby_winjob::JobObject, labby_winjob::WinJobError> {
    labby_winjob::JobObject::assign(child.id())
}
```

The caller keeps the returned job, calls `close()` during explicit shutdown,
and waits/reaps its child using the host's process API. README code fences are
explanatory and are not compiled as Rustdoc doctests.

`pid_liveness` distinguishes `Alive`, `Exited`, and `NotFound` from query errors.
Use that typed result when proving cleanup. `pid_is_alive` collapses query
errors to `false`; it cannot prove that an inaccessible process has exited.
`find_first_child_pid` is a process-snapshot helper, with `None` covering both
no observed child and snapshot failure. `WinJobError` preserves the operation
name and Win32 error code for host diagnostics.

## Verified filesystem handles

[`fs`](src/fs.rs) exposes owned `std::fs::File` guards and operations borrowing
`&File`. Keep those values alive throughout verification and the action they
protect. The helpers do not interpret credential contents, bootstrap journals,
or a product's authority to mutate an existing object.

| API | Contract |
| --- | --- |
| `AncestorGuard::for_file` | Pin an absolute path's ancestors against rename/replacement and reject reparse-point/non-directory ancestors |
| `open_read` | Open a regular final entry without following reparse points; reject hard links and deny concurrent write/delete sharing |
| `open_directory` | Open a directory without following or permitting replacement of its entry |
| `identity` | Read the exact handle's volume, full 128-bit file ID, and link count; preserve the full ID for ReFS |
| `delete_on_close` | Mark the verified regular-file handle for deletion; require delete access and release the handle afterward |
| `open_sqlite_verification` | Pin the SQLite file and ancestors while permitting SQLite writes; expose the pinned file through `file()` |
| `publish_file_no_replace` | Move a verified regular temporary to an absolute same-directory destination without replacing an existing winner |

`open_read` protects the final entry; callers separately retain an
`AncestorGuard` when ancestor stability is required. Its parent identity and
`verify_parent_acl()` support checks before protected creation/read/replacement
or deletion. Never verify one handle and then delete a path that could resolve
to another object. The product's
[`installation/secure_file.rs`](../labby/src/installation/secure_file.rs) shows
identity/content checks followed by exact-handle deletion.

SQLite verification deliberately allows concurrent writes. It establishes
identity and sharing constraints, not a stable database-content snapshot.
Callers must retain the guard while using pathname-based SQLite, verify the
needed ACL, and use SQLite transactions for content validation.

No-replace publication returns `Ok(true)` when moved, `Ok(false)` when a
destination already exists, and an I/O error otherwise. It does not create the
temporary, decide its content/permission policy, or replace a winner. The move
avoids a hard-link publication phase; callers still own synchronization,
identity continuity, and post-publication verification required by their flow.

## Ownership and DACL primitives

Filesystem verification and mutation return `std::io::Result`. Verification
helpers inspect native owner/DACL state without rewriting an existing object
to make it acceptable. `verify_private_acl` checks the protected current-user
file policy; `verify_directory_acl` rejects foreign write/delete-child authority
while allowing the trusted Windows principals defined in the implementation.

`set_created_owner` is for caller-created objects: it pins the path, reopens it,
checks the full original identity, and assigns current-user ownership.
`harden_current_user_dacl` changes only the DACL and preserves ownership; an
optional original handle binds the mutation to that identity.
`verify_current_user_only_dacl` checks that DACL without imposing ownership.
Directory-specific hardening/verification helpers also establish the intended
child-inheritance policy. The auth/product layer chooses when each operation is
authorized and which content, owner, ACL, and recovery policy applies.

## Native verification and prerequisites

Use a real Windows Rust environment with the workspace
[toolchain](../../rust-toolchain.toml), target linker/system libraries, and filesystem
support for the native identity/sharing/security APIs. Filesystem tests create
local files/directories and a junction through `cmd.exe`; process tests use
`cmd` and `ping`. Read the [testing guide](../../docs/dev/TESTING.md) and
[crate instructions](AGENTS.md) before changing this boundary.

On Windows, from the repository root:

```powershell
cargo nextest run -p labby-winjob
cargo clippy -p labby-winjob --all-targets -- -D warnings
```

The crate's [filesystem tests](src/fs.rs) cover no-replace publication, native
DACL behavior, ownership preservation, hard-link rejection, identity mismatch,
and junction ancestors. The product's
[Windows reaping test](../labby/tests/windows_job_object_reaping.rs) is gated on
Windows and the product `gateway` feature, and is ignored in the ordinary suite.
Run its production-guard child/grandchild fixture explicitly on Windows:

```powershell
cargo nextest run -p labby --all-features --test windows_job_object_reaping --run-ignored ignored-only
```

Cross-compilation can check target compilation; it cannot prove native cleanup,
sharing, ACL, or filesystem behavior. Report the native target and executed
tests separately from non-Windows or compile-only results.
