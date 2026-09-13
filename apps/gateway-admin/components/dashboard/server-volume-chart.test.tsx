import test from 'node:test'
import assert from 'node:assert/strict'
import { combineServerVolume } from './server-volume-chart'

test('server series retain bucket alignment and compute only the unassigned remainder', () => {
  const result = combineServerVolume([{ ts: 1000, calls: 12, failed: 1 }, { ts: 2000, calls: 6, failed: 0 }], ['first', 'second'], [
    { timeseries: [{ ts_unix: 1, calls: 5, failed: 0 }, { ts_unix: 2, calls: 1, failed: 0 }] },
    { timeseries: [{ ts_unix: 1, calls: 4, failed: 1 }, { ts_unix: 2, calls: 3, failed: 0 }] },
  ])
  assert.deepEqual(result, { names: ['first', 'second', 'Other'], buckets: [{ ts: 1000, total: 12, values: [5, 4, 3] }, { ts: 2000, total: 6, values: [1, 3, 2] }] })
})

test('missing, misaligned or inconsistent upstream responses do not invent Other values', () => {
  const total = [{ ts: 1000, calls: 5, failed: 0 }]
  assert.throws(() => combineServerVolume(total, ['first'], []), /incomplete/)
  assert.throws(() => combineServerVolume(total, ['first'], [{ timeseries: [] }]), /different time buckets/)
  assert.throws(() => combineServerVolume(total, ['first'], [{ timeseries: [{ ts_unix: 2, calls: 3, failed: 0 }] }]), /incomplete/)
  assert.throws(() => combineServerVolume(total, ['first'], [{ timeseries: [{ ts_unix: 1, calls: 6, failed: 0 }] }]), /changed while loading/)
})
