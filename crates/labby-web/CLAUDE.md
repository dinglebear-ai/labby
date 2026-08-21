# labby-web — Static Asset Runtime

This crate owns only gateway-admin static asset embedding, filesystem resolution, and content-type/cache metadata.

It returns surface-neutral asset data. Axum routing, auth policy, node/runtime policy, and product SPA ordering belong in the consuming Labby product code.

Preserve path traversal and symlink-escape protections. File-like missing paths must not silently become SPA fallbacks. Keep cache policy conservative for HTML/install scripts and immutable only where safe.
