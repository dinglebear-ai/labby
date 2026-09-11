import assert from 'node:assert/strict'
import test from 'node:test'
import { renderToStaticMarkup } from 'react-dom/server'
import { ToolVolumeChart } from './tool-volume-chart'

test('volume chart uses the reference height and endpoint-only time labels', () => {
  const start = Date.UTC(2026, 8, 9, 0)
  const end = start + 3600000
  const html = renderToStaticMarkup(<ToolVolumeChart window="24h" data={[
    { ts: start, calls: 10, failed: 1 }, { ts: end, calls: 20, failed: 0 },
  ]} />)
  assert.match(html, /h-\[210px\]/)
  assert.match(html, /aria-label="Chart time range"/)
  for (const ts of [start, end]) assert.ok(html.includes(new Date(ts).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })))
})

test('an empty time series does not invent a chart range', () => {
  const html = renderToStaticMarkup(<ToolVolumeChart window="24h" data={[]} />)
  assert.doesNotMatch(html, /Chart time range/)
})
