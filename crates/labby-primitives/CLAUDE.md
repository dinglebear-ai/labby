# labby-primitives — Leaf Vocabulary

This crate is the dependency-leaf shared vocabulary layer. It must have no internal workspace dependencies.

Own only small stable types that genuinely need to be shared across otherwise-independent crates, including action/parameter metadata, plugin/UI metadata, MCP constants, and static SSRF primitives.

Do not add product routing, async runtimes, transport clients, config/env reads, storage, or presentation code here. A type should not move into `labby-primitives` merely to avoid a dependency decision.

Public types are contract-heavy because several crates consume them. Preserve serialization, safety semantics, and docs when changing them.
