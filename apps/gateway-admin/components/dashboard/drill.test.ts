import test from 'node:test'
import assert from 'node:assert/strict'

import {
  actorDrillTarget,
  actorUsageHref,
  sameAttributionDrillFilter,
} from './drill.ts'

test('actor drill targets preserve exact client and agent dimensions', () => {
  assert.deepEqual(actorDrillTarget({
    id: 'client-row',
    filter_id: 'unattributed',
    label: 'Codex CLI',
    kind: 'client',
    attribution: { client_name: 'Codex CLI', client_version: '1.2' },
  }), {
    type: 'agent',
    filter: { actor: 'unattributed', client_name: 'Codex CLI', client_version: '1.2' },
    label: 'Codex CLI',
    kind: 'client',
  })

  assert.deepEqual(actorDrillTarget({
    id: 'agent-row',
    filter_id: 'sub:owner',
    label: 'daily review',
    kind: 'agent',
    attribution: { agent_id: 'daily-review' },
  }), {
    type: 'agent',
    filter: { actor: 'sub:owner', agent_id: 'daily-review' },
    label: 'daily review',
    kind: 'agent',
  })
})

test('subject and legacy drills retain the actor filter only', () => {
  assert.deepEqual(actorDrillTarget({
    id: 'subject-row',
    filter_id: 'sub:verified',
    label: 'Subject verified',
    kind: 'subject',
  }).filter, { actor: 'sub:verified' })
  assert.deepEqual(actorDrillTarget({
    id: 'legacy-tag',
    label: 'legacy-tag',
    kind: 'unknown',
  }).filter, { actor: 'legacy-tag' })
})

test('usage explorer links preserve exact attribution filters', () => {
  assert.equal(actorUsageHref({
    type: 'agent',
    filter: { actor: 'unattributed', client_name: 'Codex CLI', client_version: '1.2 beta' },
    label: 'Codex CLI',
    kind: 'client',
  }, '24h'), '/usage?window=24h&agent=unattributed&client_name=Codex+CLI&client_version=1.2+beta')
})

test('same-principal clients remain distinct drawer identities', () => {
  assert.equal(sameAttributionDrillFilter(
    { actor: 'sub:shared', client_name: 'Codex CLI', client_version: '1.0' },
    { actor: 'sub:shared', client_name: 'Codex CLI', client_version: '2.0' },
  ), false)
  assert.equal(sameAttributionDrillFilter(
    { actor: 'sub:shared', agent_id: 'review-agent' },
    { actor: 'sub:shared', agent_id: 'review-agent' },
  ), true)
})
