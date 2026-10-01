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
  assert.equal(shouldRetryMetrics(new DOMException('Authority changed', 'AbortError')), false)
})


test('permanent telemetry failures stop automatic retries', () => {
  for (const status of [400, 401, 403, 409, 422]) assert.equal(shouldRetryMetrics(Object.assign(new Error('permanent'), { status })), false)
  for (const status of [408, 429, 503]) assert.equal(shouldRetryMetrics(Object.assign(new Error('transient'), { status })), true)
})
