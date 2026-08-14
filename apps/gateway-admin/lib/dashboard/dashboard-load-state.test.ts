import test from 'node:test'
import assert from 'node:assert/strict'

import {
  metricsLoadState,
  shouldRetryMetrics,
} from './dashboard-load-state.ts'

test('metrics failure is terminal instead of remaining in the loading state', () => {
  const error = Object.assign(new Error('request failed with status 500'), { status: 500 })

  assert.equal(metricsLoadState(undefined, error, false), 'error')
})

test('unsupported metrics endpoint has a distinct unavailable state', () => {
  const error = Object.assign(new Error('request failed with status 404'), { status: 404 })

  assert.equal(metricsLoadState(undefined, error, false), 'unavailable')
})

test('unsupported metrics errors are never retried', () => {
  const notFound = Object.assign(new Error('not found'), { status: 404 })
  const unknownAction = Object.assign(new Error('unknown action'), {
    status: 400,
    code: 'unknown_action',
  })

  assert.equal(shouldRetryMetrics(notFound), false)
  assert.equal(shouldRetryMetrics(unknownAction), false)
  assert.equal(shouldRetryMetrics(new Error('temporary failure')), true)
})
