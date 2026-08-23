# Provider-neutral Skills core implementation plan

## Slice 1: explain current availability

- [x] Introduce `SkillExposureDecision` in the gateway runtime.
- [x] Compute it from the same fail-closed policy used for enforcement.
- [x] Replace `OperatorSkill.exposed` storage with the decision.
- [x] Project structured decision data and retain derived `exposed` JSON.
- [x] Add unit tests for allow-all, matched-pattern, and not-matched decisions.

## Slice 2: dedicated policy vocabulary

- [x] Introduce `SkillExposurePolicy` without changing persisted config syntax.
- [x] Compile legacy `expose_skills` patterns through the shared matcher into
  the new policy.
- [x] Project non-mutating per-skill evaluation through operator decisions.
- [x] Keep list and direct-get/read enforcement symmetric.

## Slice 3: canonical descriptor and identity

- [x] Define provider-scoped `SkillId`, `SkillProviderId`, and compact
  `SkillDescriptor` in the lowest shared crate used by providers and product
  dispatch.
- [x] Add a fail-closed compatibility availability summary once the provider
  result contract established its exact inputs.
- [ ] Add a distinct requirements summary once concrete provider inputs require
  it.
- [x] Adapt existing `ValidatedSkill` without duplicating validation or fetching
  bodies.
- [x] Preserve existing published/source URI behavior at surface boundaries.

## Slice 4: provider abstraction

- [x] Define bounded discovery/get/read provider contracts.
- [x] Wrap the existing SEP-2640 path as the first remote provider.
- [x] Add bundled and operator-local providers over the same descriptor contract.
- [x] Preserve cache, subject, integrity, and route-scope behavior.

## Slice 5: policy composition and loadouts

- Compose upstream policy with user/project/group/loadout selection.
- Return a decision trace suitable for preview and operator remediation.
- Keep access authorization outside this subsystem.

## Slice 6: compatibility and ownership

- [x] Define provider-neutral classifications for supported, preserved hint,
  adaptable, dependency unavailable, invalid, and policy blocked states.
- Classify concrete vendor fields as supported, preserved hint, adaptable, dependency
  unavailable, invalid, or policy blocked.
- Add explicit Artifact save/fork/customize/pin operations.
- Add opt-in filesystem projection only after the core contract is stable.

## Verification gates

For each slice: focused tests first, then `cargo test -p labby-gateway` and
`cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings` for
gateway changes. Run runtime gates when shared vocabulary changes, regenerate
catalogs for action/schema changes, and verify real operator JSON plus browser
behavior before calling a UI milestone complete.
