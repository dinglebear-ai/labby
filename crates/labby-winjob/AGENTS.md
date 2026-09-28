# labby-winjob — Windows Process Containment

This crate is the workspace's sanctioned unsafe boundary for Windows Job Object FFI used to reap spawned process trees and the small `fs` module used for verified bootstrap file identity, handle-based deletion, and native DACL operations.

Keep the public API safe. Raw `windows-sys` calls and required `unsafe` stay encapsulated here; do not move them into `labby` or `labby-gateway`.

Changes must preserve kill-on-job-close semantics and be verified on Windows-specific tests/builds in addition to normal workspace checks.

Filesystem helpers expose only safe owned/borrowed handle operations. Keep ACL/content policy in the product or reusable auth layer. Never replace verified-handle deletion with a pathname delete; pin ancestors, reject reparse points/hard links, and preserve the full 128-bit file ID required by ReFS.
