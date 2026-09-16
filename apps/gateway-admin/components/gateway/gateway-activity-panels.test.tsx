import test from 'node:test'
import assert from 'node:assert/strict'
import { gatewayCallSeries } from './gateway-activity-panels'

test('server activity preserves chronological buckets across midnight and separates outcomes', () => {
  const result = gatewayCallSeries({ timeseries: [
    { ts_unix: 1_800_003_600, calls: 7, failed: 2 },
    { ts_unix: 1_800_000_000, calls: 3, failed: 1 },
  ] })
  assert.deepEqual(result, [
    { ts: 1_800_000_000_000, calls: 3, failed: 1, succeeded: 2 },
    { ts: 1_800_003_600_000, calls: 7, failed: 2, succeeded: 5 },
  ])
})
