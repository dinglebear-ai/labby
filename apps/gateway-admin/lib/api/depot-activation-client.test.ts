import test from 'node:test'
import assert from 'node:assert/strict'
import { createDepotSkillActivationAttempt, prepareDepotSkillActivation } from './depot-activation-client'
import { __setBrowserSessionStateForTests } from '../auth/session-store'

const source = { connection_id: 'catalog', artifact_id: 'skill-one', revision_id: 'revision-one' }
const response = (data: unknown) => new Response(JSON.stringify(data))
const summary = (active = false, library_version = 3) => ({
  library_version,
  artifact_id: source.artifact_id, archived: false, materialized: true,
  latest_revision_id: source.revision_id, active_revision_id: active ? source.revision_id : null,
  published_library_version: library_version, allowed_actions: ['artifacts.activate'],
})
// MutationReceipt is flat (unlike the erroneous nested get fixture caught below).
const receipt = () => ({
  outcome: 'committed', artifact_id: source.artifact_id, active_revision_id: source.revision_id,
  canonical_uri: 'skill://labby/skill-one/SKILL.md', old_generation: 3, new_generation: 4,
  committed_library_version: 4, published_library_version: 4, library_digest: `sha256:${'b'.repeat(64)}`,
  rejected_entries: { items: [] }, relist_required: false,
  relist_guidance: 'Re-run skills.list or native skills/list; Labby does not emit a Skills list_changed notification.',
  list_changed_notification: false,
})
const membership = () => ({ library_version: 3, items: [{ ...source, status: 'exact_revision_present' }] })

test('artifacts.get accepts the flattened Rust VersionedSkillLibrarySummary contract and rejects a nested item envelope', async () => {
  const old = globalThis.fetch
  // Field layout follows skill_library/types.rs VersionedSkillLibrarySummary #[serde(flatten)]
  // and dispatch.rs summary(); extra display fields are deliberately retained in this fixture.
  const rustSummary = {
    library_version: 3,
    artifact_id: 'skill-one', name: 'skill-one', archived: false,
    active_revision_id: null, latest_revision_id: 'revision-one',
    visibility: 'private', access_label: 'personal', can_mutate: true,
    owner: { relationship: 'self' }, provenance: { source: 'depot' },
    materialized: true, canonical_uri: null, current_generation: 3,
    published_library_version: 3,
    allowed_actions: ['artifacts.get', 'artifacts.read', 'artifacts.history', 'artifacts.save', 'artifacts.archive', 'artifacts.activate'],
    latest_revision_files: [{ path: 'SKILL.md', digest: `sha256:${'a'.repeat(64)}`, size: 42, media_type: 'text/markdown' }],
  }
  try {
    let nested = false
    globalThis.fetch = async (_url, init) => {
      if (JSON.parse(String(init?.body)).action === 'artifacts.depot_membership') return response(membership())
      return response(nested ? { library_version: 3, item: rustSummary } : rustSummary)
    }
    assert.equal((await prepareDepotSkillActivation('skill', source)).status, 'ready')
    nested = true
    await assert.rejects(prepareDepotSkillActivation('skill', source))
  } finally { globalThis.fetch = old }
})

test('unsupported kinds and missing exact source never call import or activation', async () => {
  const old = globalThis.fetch
  let calls = 0
  globalThis.fetch = async () => { calls += 1; throw new Error('unexpected') }
  try {
    for (const kind of ['mcp', 'agent', 'loadout']) assert.equal((await prepareDepotSkillActivation(kind, source)).status, 'blocked')
    assert.equal((await prepareDepotSkillActivation('skill')).status, 'blocked')
    assert.equal(calls, 0)
  } finally { globalThis.fetch = old }
})

test('readiness requires exact current membership, activation permission and materialized unarchived Skill', async () => {
  const old = globalThis.fetch
  try {
    for (const status of ['absent', 'different_revision_present']) {
      globalThis.fetch = async (_url, init) => {
        assert.equal(JSON.parse(String(init?.body)).action, 'artifacts.depot_membership')
        return response({ library_version: 3, items: [{ ...source, status }] })
      }
      assert.equal((await prepareDepotSkillActivation('skill', source)).status, 'blocked')
    }
    for (const override of [{ archived: true }, { materialized: false }, { allowed_actions: [] }]) {
      globalThis.fetch = async (_url, init) => JSON.parse(String(init?.body)).action === 'artifacts.depot_membership'
        ? response(membership()) : response({ ...summary(), ...override })
      assert.equal((await prepareDepotSkillActivation('skill', source)).status, 'blocked')
    }
  } finally { globalThis.fetch = old }
})

test('readiness rejects inconsistent identity/version and stale browser context', async () => {
  const old = globalThis.fetch
  try {
    for (const invalid of [summary(false, 4), { ...summary(), artifact_id: 'another' }, { ...summary(), latest_revision_id: 'another' }]) {
      globalThis.fetch = async (_url, init) => JSON.parse(String(init?.body)).action === 'artifacts.depot_membership' ? response(membership()) : response(invalid)
      await assert.rejects(prepareDepotSkillActivation('skill', source), /library changed/)
    }
    globalThis.fetch = async (_url, init) => {
      if (JSON.parse(String(init?.body)).action === 'artifacts.depot_membership') return response(membership())
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
      return response(summary())
    }
    await assert.rejects(prepareDepotSkillActivation('skill', source), /session changed/)
  } finally { globalThis.fetch = old }
})

test('activation explicitly rechecks before CAS mutation and verifies current publication', async () => {
  const old = globalThis.fetch
  const requests: Array<{ action: string; params: object }> = []
  let active = false
  globalThis.fetch = async (_url, init) => {
    const request = JSON.parse(String(init?.body))
    requests.push(request)
    if (request.action === 'artifacts.depot_membership') return response(membership())
    if (request.action === 'artifacts.get') return response(summary(active, active ? 4 : 3))
    assert.equal(request.action, 'artifacts.activate')
    active = true
    return response(receipt())
  }
  try {
    const prepared = await prepareDepotSkillActivation('skill', source)
    assert.equal(prepared.status, 'ready')
    if (prepared.status !== 'ready') throw new Error('expected ready')
    assert.equal(requests.length, 2, 'preparation is read-only')
    await createDepotSkillActivationAttempt(prepared, 'stable-confirmed-key').run()
    assert.deepEqual(requests.map(request => request.action), ['artifacts.depot_membership', 'artifacts.get', 'artifacts.depot_membership', 'artifacts.get', 'artifacts.activate', 'artifacts.get'])
    assert.deepEqual(requests[4].params, { artifact_id: source.artifact_id, expected_revision_id: source.revision_id, expected_library_version: 3, idempotency_key: 'stable-confirmed-key' })
  } finally { globalThis.fetch = old }
})

test('uncertain activation retry preserves the entire mutation and deduplicates simultaneous clicks', async () => {
  const old = globalThis.fetch
  const writes: unknown[] = []
  let active = false
  globalThis.fetch = async (_url, init) => {
    const request = JSON.parse(String(init?.body))
    if (request.action === 'artifacts.depot_membership') return response(membership())
    if (request.action === 'artifacts.get') return response(summary(active, active ? 4 : 3))
    assert.equal(request.action, 'artifacts.activate')
    writes.push(request)
    if (writes.length === 1) throw new Error('response lost')
    active = true
    return response({ ...receipt(), outcome: 'replayed' })
  }
  try {
    const prepared = await prepareDepotSkillActivation('skill', source)
    if (prepared.status !== 'ready') throw new Error('expected ready')
    const attempt = createDepotSkillActivationAttempt(prepared, 'retry-key')
    const first = attempt.run()
    assert.equal(attempt.run(), first)
    await assert.rejects(first, /response lost/)
    await attempt.run()
    assert.equal(writes.length, 2)
    assert.deepEqual(writes[0], writes[1])
  } finally { globalThis.fetch = old }
})

test('changed confirmation state and session epoch prevent activation writes', async () => {
  const old = globalThis.fetch
  let changed = false
  let writes = 0
  globalThis.fetch = async (_url, init) => {
    const request = JSON.parse(String(init?.body))
    if (request.action === 'artifacts.depot_membership') return response({ ...membership(), library_version: changed ? 4 : 3 })
    if (request.action === 'artifacts.get') return response(summary(false, changed ? 4 : 3))
    writes += 1
    return response(receipt())
  }
  try {
    const prepared = await prepareDepotSkillActivation('skill', source)
    if (prepared.status !== 'ready') throw new Error('expected ready')
    changed = true
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /state changed/)
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /session changed/)
    assert.equal(writes, 0)
  } finally { globalThis.fetch = old }
})

test('old replay or unconfirmed publication never reports current activation success', async () => {
  const old = globalThis.fetch
  try {
    for (const invalidReceipt of [{ ...receipt(), published_library_version: 3 }, { ...receipt(), active_revision_id: 'other' }]) {
      globalThis.fetch = async (_url, init) => {
        const { action } = JSON.parse(String(init?.body))
        return response(action === 'artifacts.depot_membership' ? membership() : action === 'artifacts.get' ? summary() : invalidReceipt)
      }
      const prepared = await prepareDepotSkillActivation('skill', source)
      if (prepared.status !== 'ready') throw new Error('expected ready')
      await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /not confirmed/)
    }
    let activated = false
    globalThis.fetch = async (_url, init) => {
      const { action } = JSON.parse(String(init?.body))
      if (action === 'artifacts.activate') { activated = true; return response(receipt()) }
      return response(action === 'artifacts.depot_membership' ? membership() : summary(false, activated ? 4 : 3))
    }
    const prepared = await prepareDepotSkillActivation('skill', source)
    if (prepared.status !== 'ready') throw new Error('expected ready')
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /no longer confirms/)
  } finally { globalThis.fetch = old }
})

test('mutation auth errors are structured and never refresh or replay automatically', async () => {
  const old = globalThis.fetch
  const actions: string[] = []
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body))
    actions.push(action)
    if (action === 'artifacts.depot_membership') return response(membership())
    if (action === 'artifacts.get') return response(summary())
    return new Response(JSON.stringify({ kind: 'auth_failed', message: 'Activation permission revoked', param: 'actor' }), { status: 403 })
  }
  try {
    const prepared = await prepareDepotSkillActivation('skill', source)
    if (prepared.status !== 'ready') throw new Error('expected ready')
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), error => error instanceof Error && 'status' in error && error.status === 403 && 'code' in error && error.code === 'auth_failed')
    assert.equal(actions.filter(action => action === 'artifacts.activate').length, 1)
    assert.ok(actions.every(action => action.startsWith('artifacts.')))
  } finally { globalThis.fetch = old }
})

test('permission revoked at confirmation and authority changed during mutation response are rejected', async () => {
  const old = globalThis.fetch
  let revoke = false
  let writes = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body))
    if (action === 'artifacts.depot_membership') return response(membership())
    if (action === 'artifacts.get') return response({ ...summary(), allowed_actions: revoke ? [] : ['artifacts.activate'] })
    writes += 1
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    return response(receipt())
  }
  try {
    const prepared = await prepareDepotSkillActivation('skill', source)
    if (prepared.status !== 'ready') throw new Error('expected ready')
    revoke = true
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /state changed/)
    assert.equal(writes, 0)
    revoke = false
    await assert.rejects(createDepotSkillActivationAttempt(prepared).run(), /session changed/)
    assert.equal(writes, 1)
  } finally { globalThis.fetch = old }
})
