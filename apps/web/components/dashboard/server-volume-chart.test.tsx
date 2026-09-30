import test from 'node:test'
import assert from 'node:assert/strict'
import { combineServerVolume, serverVolumeKey, SERVER_VOLUME_REFRESH_MS } from './server-volume-chart'

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


test('server breakdown cache stays bounded and inactive charts cannot request data', () => {
  assert.equal(serverVolumeKey(undefined, '/v1', 'team-a', 1), null)
  const key = serverVolumeKey('1h', '/v1', 'team-a', 1)
  assert.deepEqual(key, ['overview-server-volume', '/v1', 'team-a', 1, '1h'])
  assert.deepEqual(serverVolumeKey('1h', '/v1', 'team-a', 1), key)
  assert.notDeepEqual(serverVolumeKey('24h', '/v1', 'team-a', 1), key)
  assert.notDeepEqual(serverVolumeKey('1h', '/v1', 'team-b', 1), key)
  assert.notDeepEqual(serverVolumeKey('1h', '/alternate', 'team-a', 1), key)
  assert.notDeepEqual(serverVolumeKey('1h', '/v1', 'team-a', 2), key)
  assert.equal(SERVER_VOLUME_REFRESH_MS, 60_000)
})
