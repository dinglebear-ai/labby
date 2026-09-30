---
title: "Issues"
created: "2026-09-26"
updated: "2026-09-26"
---

# Issues

| Finding | Disposition |
| --- | --- |
| Original staging was only a static workflow fixture | Corrected the evidence classification; new fixture exposes exact commit/artifact identity |
| MCP launch adapter rejects runtime 0.7.3 | Still unresolved in that adapter; native CLI route independently verified, no compatibility guard removed |
| Host gh token invalid | Not repaired or copied; existing authorized SSH broker selected and dry-run verified |
| Base image lacks project development tools | Complete reference package lock and verifying bootstrap added |
| Snapshot does not prove complete runtime configuration | Promotion uses a fresh pinned base and explicit configuration; snapshots remain checkpoints |
| Original dev sandbox has a 24-hour expiry | Existing configuration preserved; new retained staging must have explicit unlimited duration/idle retention |
| HTTPError responses were not explicitly closed | Fixed and the real-HTTP tests rerun without those warnings |
| Global artifact guidance points at retired Dendrite paths | Observed only; unrelated symlinks and other sessions’ work left untouched |

A native runtime success must not be reported as a successful MCP start test.

## Post-publication findings
- Repository Contract rejected missing title/created/updated metadata on the seven task records. Added the required frontmatter rather than exempting the documents.
- A real stop/start left the staging VM running with no fixture process. Recovery requires explicit service-command relaunch through sandbox_exec_start followed by an external identity check. Native VM start is not a service supervisor.
