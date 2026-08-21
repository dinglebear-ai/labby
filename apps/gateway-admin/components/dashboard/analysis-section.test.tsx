import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { AnalysisSection } from './analysis-section.tsx'
import { aggregateGatewayUsage } from '@/lib/dashboard/gateway-usage-adapter'

test('analysis labels uncollected dimensions instead of rendering fake zero metrics', () => {
  const metrics = aggregateGatewayUsage(
    '24h',
    1_800_000_000_000,
{
      window_total_calls: 0,
      total_calls: 0,
      error_calls: 0,
      avg_elapsed_ms: 0,
      p50_elapsed_ms: 0,
      p95_elapsed_ms: 0,
      p99_elapsed_ms: 0,
      distinct_tools: 0,
      distinct_actors: 0,
      peak_per_min: 0,
      top_tools: [],
      least_tools: [],
      top_actors: [],
      slowest_tools: [],
      errors: [],
      upstreams: [],
      hourly: [],
      timeseries: [],
      facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
    },
  )

  const html = renderToStaticMarkup(
    <AnalysisSection metrics={metrics} onSelectTool={() => undefined} />,
  )

  assert.match(html, /Surface attribution is not collected/)
  assert.match(html, /Token usage is not collected/)
  assert.doesNotMatch(html, /0 new/)
})
