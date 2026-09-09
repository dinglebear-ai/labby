import test from 'node:test'
import assert from 'node:assert/strict'
import { createOverviewFixtureMetrics } from './overview-fixture-data.mjs'

const args = { since_unix: 1700000000, until_unix: 1700086400, bucket_count: 24 }
const sum = (items, field) => items.reduce((total, item) => total + item[field], 0)
test('synthetic metrics are deterministic and coherent across every chart dimension', () => {
  const metrics = createOverviewFixtureMetrics(args)
  assert.deepEqual(metrics, createOverviewFixtureMetrics(args))
  for (const items of [metrics.top_tools, metrics.least_tools, metrics.top_actors, metrics.upstreams, metrics.hourly, metrics.timeseries]) {
    assert.equal(sum(items, 'calls'), metrics.total_calls)
  }
  for (const items of [metrics.top_tools, metrics.upstreams, metrics.timeseries]) assert.equal(sum(items, 'failed'), metrics.error_calls)
  assert.equal(sum(metrics.errors, 'calls'), metrics.error_calls)
  assert.equal(metrics.window_total_calls, metrics.total_calls)
  assert.equal(metrics.distinct_tools, metrics.facets.tools.length)
  assert.equal(metrics.distinct_actors, metrics.top_actors.length)
  assert.ok(metrics.upstreams.every(item => item.upstream.startsWith('Fixture ')))
  assert.ok(metrics.top_actors.every(item => item.actor.startsWith('Fixture ')))
  assert.ok(metrics.timeseries.every(item => item.failed <= item.calls))
})

test('bucket generation is bounded and strictly inside the requested time window', () => {
  for (const bucket_count of [-100, 0, 1, 12, 100000]) {
    const metrics = createOverviewFixtureMetrics({ ...args, bucket_count })
    assert.ok(metrics.timeseries.length >= 1 && metrics.timeseries.length <= 24)
    const stamps = metrics.timeseries.map(item => item.ts_unix)
    assert.equal(new Set(stamps).size, stamps.length)
    assert.ok(stamps.every(stamp => stamp >= args.since_unix && stamp < args.until_unix))
  }
  assert.equal(createOverviewFixtureMetrics({ since_unix: 1, until_unix: 3 }).timeseries.length, 2)
  for (const invalid of [
    { ...args, since_unix: -1 }, { ...args, until_unix: args.since_unix },
    { ...args, bucket_count: Infinity }, { ...args, bucket_count: 1.5 },
    { ...args, until_unix: Number.MAX_SAFE_INTEGER },
  ]) assert.throws(() => createOverviewFixtureMetrics(invalid), RangeError)
})
