# Microsandbox implementation process

## Objective
Extract and prove a reusable, evidence-backed implementation workflow. This is not an implementation of the entire Labby product.

## Acceptance criteria
- Persistent development sandbox and Git branch; no host worktree edits.
- Preflight runtime, adapter, bootstrap, Git publication, and target isolation.
- Research, plans, progress, issues, changelog, and reflection artifacts.
- Test-first receipt gates plus real HTTP success/failure checks.
- Exact-commit, checksum-verified fresh staging retained separately from production.
- Authorized commit/push/PR with no merge or production deployment.
- No Claude Agents, agent workflows, or exGPT.

## Lanes
Implementation and tests share one sequential writer. Source/documentation research and read-only runtime checks can run independently. Publication and staging depend on the final verified commit.
