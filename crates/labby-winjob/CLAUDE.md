# labby-winjob — Windows Process Containment

This crate is the workspace's sanctioned unsafe boundary for Windows Job Object FFI used to reap spawned process trees.

Keep the public API safe. Raw `windows-sys` calls and required `unsafe` stay encapsulated here; do not move them into `labby` or `labby-gateway`.

Changes must preserve kill-on-job-close semantics and be verified on Windows-specific tests/builds in addition to normal workspace checks.
