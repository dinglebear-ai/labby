import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { AnalysisSection } from './analysis-section.tsx'
import { aggregateGatewayUsage } from '@/lib/dashboard/gateway-usage-adapter'

test('analysis labels uncollected dimensions instead of rendering fake zero metrics', () => {
  const metrics = aggregateGatewayUsage(
    '24h',
    1_800_000_000_000,
    { total_calls: 0, error_calls: 0, avg_elapsed_ms: 0, top_tools: [], top_actors: [] },
    { calls: [], total_matching: 0, next_cursor: null },
  )

  const html = renderToStaticMarkup(
    <AnalysisSection metrics={metrics} onSelectTool={() => undefined} />,
  )

  assert.match(html, /Surface attribution is not collected/)
  assert.match(html, /Token usage is not collected/)
  assert.doesNotMatch(html, /0 new/)
})
