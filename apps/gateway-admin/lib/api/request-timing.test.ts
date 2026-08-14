import test from 'node:test'
import assert from 'node:assert/strict'

import { withRequestTiming } from './request-timing.ts'

test('request timing reports a successful request', async () => {
  const reports: Array<{ name: string; outcome: string; elapsedMs: number }> = []

  const result = await withRequestTiming('gateway.list', async () => 'ok', (report) => {
    reports.push(report)
  })

  assert.equal(result, 'ok')
  assert.equal(reports.length, 1)
  assert.equal(reports[0].name, 'gateway.list')
  assert.equal(reports[0].outcome, 'success')
  assert.ok(reports[0].elapsedMs >= 0)
})

test('request timing reports and rethrows a failed request', async () => {
  const reports: Array<{ name: string; outcome: string; elapsedMs: number }> = []
  const failure = new Error('boom')

  await assert.rejects(
    withRequestTiming('logs.metrics', async () => Promise.reject(failure), (report) => {
      reports.push(report)
    }),
    failure,
  )

  assert.equal(reports.length, 1)
  assert.equal(reports[0].outcome, 'error')
})
