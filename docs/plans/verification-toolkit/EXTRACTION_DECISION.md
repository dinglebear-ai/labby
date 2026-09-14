# M8 Second-Adopter and Extraction Decision

Decision date: 2026-09-13.

## Decision

Keep the verification toolkit incubating inside Labby. Do not extract it to a
standalone repository and do not extract `verify-macros`.

Labby is the only demonstrated adopter. The proposed second adopter, Unraid
Core / the shared mount core, has not adopted the toolkit, and this milestone
does not authorize changes to that separately owned repository. A read-only
search of the candidate checkout at revision
`427f2e118c848e400173e9e60332235c0132455e` found no `verify-core`,
`verify-scenario`, invariant-catalog schema, or toolkit workflow consumption.
The Labby checkout evaluated for this decision was based on revision
`61e9fa4e1e060dc2b0e9347ab48ec2e2b277d9c9` with the in-progress toolkit
changes present in its working tree.

## Extraction Gates

| Gate | Result | Evidence |
| --- | --- | --- |
| Two adopting projects | Not met | Labby is the sole demonstrated adopter; the Core candidate has no adoption. |
| No L1 change required by the second adopter | Not evaluated | There is no second implementation from which to establish this. |
| Schema version 1 unchanged across both | Not evaluated | Only the Labby schema-v1 consumer exists. |
| CI workflows consumed with `uses:` rather than copied | Not met | No second adopter consumes the workflows. |

The correct extraction verdict is therefore **premature**. An eventual Core or
mount-core adoption is separate work requiring that repository's authorization,
its own model and scenarios, callable workflow evidence, and a new evaluation of
all four gates. Until then, the existing in-repository workspace and schemas are
the canonical implementation.

## Scope and Non-Claims

- No files in the candidate repository were changed.
- No standalone toolkit repository was created.
- No shared schema, L1 interface, or workflow contract was changed to manufacture
  an adoption result.
- This decision does not count model or replay evidence as Labby product E2E
  qualification.
