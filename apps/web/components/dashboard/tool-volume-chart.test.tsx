import assert from 'node:assert/strict'
import test from 'node:test'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  ToolVolumeChart,
  ToolVolumeLegend,
  bucketDrillRange,
  buildOutcomeRows,
  chartTickIndexes,
} from './tool-volume-chart'

test('volume chart uses the reference height and five distributed time labels', () => {
  const start = Date.UTC(2026, 8, 9, 0)
  const data = Array.from({ length: 9 }, (_, index) => ({
    ts: start + index * 3600000,
    calls: 10 + index,
    failed: 1,
  }))
  const html = renderToStaticMarkup(<ToolVolumeChart window="24h" data={data} />)

  assert.match(html, /h-\[210px\]/)
  assert.match(html, /aria-label="Chart time ticks"/)
  assert.deepEqual(chartTickIndexes(data.length), [0, 2, 4, 6, 8])
  for (const index of [0, 2, 4, 6, 8]) {
    assert.ok(html.includes(new Date(data[index].ts).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })))
  }
})

test('outcome rows reconcile malformed detail and retain unknown errors exactly once', () => {
  const [row] = buildOutcomeRows([{
    ts: 1_000,
    calls: 10,
    failed: 6,
    outcomes: [
      { kind: 'succeeded', count: 100 },
      { kind: 'upstream_error', count: 2 },
      { kind: 'timeout', count: 1 },
      { kind: 'response_too_large', count: 1 },
      { kind: 'connection_failed', count: 1 },
      { kind: 'future_error_kind', count: 100 },
      { kind: 'timeout', count: -4 },
    ],
  }], '24h')

  assert.equal(row.succeeded, 4)
  assert.equal(row.upstreamError, 2)
  assert.equal(row.timedOut, 1)
  assert.equal(row.responseTooLarge, 1)
  assert.equal(row.connectionFailed, 1)
  assert.equal(row.unknown, 1)
  assert.equal(
    row.succeeded + row.upstreamError + row.timedOut + row.responseTooLarge
      + row.connectionFailed + row.unknown + row.unclassifiedFailed,
    row.calls,
  )
})

test('older gateway buckets keep failures explicitly unclassified', () => {
  const [row] = buildOutcomeRows([{ ts: 1_000, calls: 5, failed: 3 }], '24h')
  assert.equal(row.succeeded, 2)
  assert.equal(row.unclassifiedFailed, 3)
  assert.equal(row.unknown, 0)

  const html = renderToStaticMarkup(<ToolVolumeLegend data={[{ ts: 1_000, calls: 5, failed: 3 }]} mode="outcomes" />)
  assert.match(html, /Succeeded/)
  assert.match(html, /Failed \(unclassified\)/)
  assert.doesNotMatch(html, /Upstream Error/)
})

test('outcome legend exposes real multicolor categories present in the series', () => {
  const html = renderToStaticMarkup(<ToolVolumeLegend data={[{
    ts: 1_000,
    calls: 6,
    failed: 5,
    outcomes: [
      { kind: 'succeeded', count: 1 },
      { kind: 'upstream_error', count: 1 },
      { kind: 'timeout', count: 1 },
      { kind: 'response_too_large', count: 1 },
      { kind: 'connection_failed', count: 1 },
      { kind: 'unknown', count: 1 },
    ],
  }]} mode="outcomes" />)

  for (const label of ['Succeeded', 'Upstream Error', 'Timed Out', 'Response Too Large', 'Connection Failed', 'Other / Unknown']) {
    assert.match(html, new RegExp(label.replace('/', '\\/')))
  }
})

test('bucket drill ranges use the next persisted boundary and inclusive seconds', () => {
  const rows = [{ ts: 10_000 }, { ts: 17_000 }, { ts: 31_000 }]
  assert.deepEqual(bucketDrillRange(rows, 0, '24h'), [10_000, 16_000])
  assert.deepEqual(bucketDrillRange(rows, 1, '24h'), [17_000, 30_000])
  assert.deepEqual(bucketDrillRange(rows, 8, '24h'), null)
})

test('an empty time series does not invent chart ticks', () => {
  const html = renderToStaticMarkup(<ToolVolumeChart window="24h" data={[]} />)
  assert.doesNotMatch(html, /Chart time ticks/)
})
