import assert from 'node:assert/strict'
import test from 'node:test'
import { parseReadinessState, READINESS_CHECKS } from './readiness'

const pending = () => ({ ready: false, evidence_max_age_seconds: 86400, checks: READINESS_CHECKS.map((check) => ({ check, status: 'pending', verified_at: null, resource_id: null })) })

test('incomplete or fabricated readiness cannot earn a completion state', () => {
  assert.equal(parseReadinessState(pending()).ready, false)
  for (const state of [
    { ...pending(), ready: true },
    { ...pending(), checks: pending().checks.slice(1) },
    { ...pending(), checks: [...pending().checks.slice(1), pending().checks[1]] },
    { ...pending(), checks: pending().checks.map((item) => ({ ...item, status: 'verified' })) },
    { ...pending(), checks: pending().checks.map((item) => ({ ...item, status: 'deferred' })) },
  ]) assert.throws(() => parseReadinessState(state))
})

test('only an explicit application deferral can satisfy an optional check', () => {
  const state = { ready: true, evidence_max_age_seconds: 86400, checks: READINESS_CHECKS.map((check) => ({ check, status: check === 'selected_clients' ? 'deferred' : 'verified', verified_at: 100, resource_id: 'actual-result' })) }
  assert.equal(parseReadinessState(state).ready, true)
  state.checks[2].status = 'deferred'
  assert.throws(() => parseReadinessState(state))
})
