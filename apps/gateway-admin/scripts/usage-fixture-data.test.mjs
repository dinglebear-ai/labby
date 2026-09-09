import test from 'node:test'
import assert from 'node:assert/strict'
import { createUsageFixtureCalls as calls, createUsageFixtureMetrics as metrics } from './usage-fixture-data.mjs'
const base = { since_unix: 1700000000, until_unix: 1700086400 }
const sum = (rows, field) => rows.reduce((value, row) => value + row[field], 0)
test('all pages and metrics agree deterministically across supported filters', () => {
  const sample = calls(base).calls[0]
  assert.deepEqual([...new Set(calls(base).calls.map(row => row.surface))].sort(), ['api', 'cli', 'mcp', 'web'])
  for (const filter of [{}, ...['upstream', 'tool', 'capability', 'operation', 'subject_scoped', 'actor', 'outcome'].map(key => ({ [key]: sample[key] })), { outcome: 'failed' }, { search: 'corpus' }, { tool: sample.upstream + '::' + sample.tool }, { upstream: 'absent' }]) {
    const params = { ...base, ...filter, limit: 37 }
    const rows = []
    let cursor
    do { const page = calls({ ...params, cursor }); rows.push(...page.calls); cursor = page.next_cursor } while (cursor)
    const summary = metrics(params)
    assert.equal(rows.length, summary.total_calls)
    assert.equal(rows.filter(row => row.outcome !== 'ok').length, summary.error_calls)
    assert.equal(sum(summary.timeseries, 'calls'), rows.length)
    assert.equal(sum(summary.hourly, 'calls'), rows.length)
    assert.equal(sum(summary.top_tools, 'calls'), rows.length)
    assert.equal(sum(summary.upstreams, 'calls'), rows.length)
    assert.equal(sum(summary.errors, 'calls'), summary.error_calls)
    assert.equal(summary.window_total_calls, 240)
    assert.ok(rows.every(row => row.ts_unix >= base.since_unix && row.ts_unix < base.until_unix))
    assert.deepEqual(metrics(params), summary)
  }
})
test('facets describe the full bounded window and zero buckets remain empty', () => {
  const all = metrics(base)
  assert.deepEqual(metrics({ ...base, outcome: 'failed' }).facets, all.facets)
  assert.deepEqual(metrics({ ...base, bucket_count: 0 }).timeseries, [])
  assert.deepEqual(metrics({ ...base, include_facets: false }).facets.tools, [])
  assert.equal(metrics({ since_unix: 1, until_unix: 2 }).timeseries.length, 1)
})
test('invalid bounds filters page sizes and cursors fail closed', () => {
  for (const params of [{ ...base, since_unix: -1 }, { ...base, until_unix: base.since_unix }, { ...base, limit: 101 }, { ...base, cursor: 'arbitrary' }, { ...base, cursor: 'fixture:999' }, { ...base, subject_scoped: 'true' }, { ...base, search: 'x'.repeat(513) }]) assert.throws(() => calls(params))
  assert.throws(() => metrics({ ...base, bucket_count: 25 }))
  assert.throws(() => metrics({ ...base, timezone_offset_minutes: Infinity }))
})
