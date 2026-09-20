# Architecture Decision Records

Architecture Decision Records (ADRs) capture durable Labby architecture choices,
their authority boundaries, and the alternatives considered. An ADR marked
`Proposed` records a direction under review; it does not claim that the design is
accepted or implemented.

| ADR | Status | Decision |
| --- | --- | --- |
| [0001](./0001-install-labby-first-class-install-orchestrator.md) | Accepted | Make `install-labby` the first-class guided installation orchestrator |
| [0002](./0002-labby-product-contracts-over-private-depot.md) | Accepted direction; implementation is partial | Put Labby product contracts over the private Depot backend |
| [0003](./0003-code-mode-first-party-execution-plane.md) | Proposed | Use Code Mode as Labby's bounded first-party execution plane |
| [0004](./0004-skill-aware-snippets-and-execution-receipts.md) | Proposed | Resolve snippet skills through host policy and retain execution receipts |
| [0005](./0005-artifact-store-backed-local-library.md) | Proposed | Use the ArtifactStore-backed Library as Labby's local artifact authority |
