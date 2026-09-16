import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { FanOutPanel, MostActivePanel, LeastUsedPanel } from './activity-insight-panels.tsx'

test('least-used panel shows four compact supplied rows and the authoritative distinct count', () => {
  const html = renderToStaticMarkup(<LeastUsedPanel tools={Array.from({ length: 6 }, (_, index) => ({ name: `tool-${index}`, label: `Tool ${index}`, calls: index + 1, failed: 0 }))} distinct={481}/>)
  assert.match(html, /of 481 distinct/)
  assert.match(html, /Tool 3/)
  assert.doesNotMatch(html, /Tool 4/)
  assert.match(html, /4 calls/)
  assert.match(html, /bg-aurora-warn/)
})

test('actor comparison bars use reported calls with compact identity chips', () => {
  const render = (calls: number[]) => renderToStaticMarkup(<MostActivePanel actors={{
    agent: { active: calls.length, top: calls.map((count, index) => ({ id: `actor-${index}`, label: index ? 'Claude Code' : 'Codex CLI', kind: 'agent' as const, calls: count })) },
    device: { active: 0, top: [] }, ip: { active: 0, top: [] },
  }} window="24h" onSelectActor={() => {}} />)
  const html = render([100, 25])
  assert.match(html, />CC<\/span>/)
  assert.match(html, /100 calls/)
  assert.match(html, /width:100%/)
  assert.match(html, /width:25%/)
  assert.match(html, /from-aurora-accent-pink-deep/)
  const emptyCounts = render([0, 0])
  assert.match(emptyCounts, /width:0%/)
  assert.doesNotMatch(emptyCounts, /NaN|Infinity/)
})

test('persisted usage labels actors as subjects and fan-out as uncollected', () => {
  const actors = {
    agent: { active: 1, top: [{ id: 'codex', label: 'codex', kind: 'agent' as const, calls: 3 }] },
    device: { active: 0, top: [] },
    ip: { active: 0, top: [] },
  }
  const subjects = renderToStaticMarkup(
    <MostActivePanel
      actors={actors}
      window="24h"
      actorKindsCollected={false}
      onSelectActor={() => undefined}
    />,
  )
  const fanOut = renderToStaticMarkup(
    <FanOutPanel
      collected={false}
      fanOut={{ runs: 0, total_calls: 0, avg_calls_per_run: 0, max_calls_in_run: 0, timeout_rate: 0, truncation_rate: 0, artifact_writes: 0 }}
    />,
  )

  assert.match(subjects, /Most active subjects/)
  assert.doesNotMatch(subjects, /Devices/)
  assert.match(fanOut, /Fan-out telemetry is not collected/)
})

test('known client and unknown identity facets use truthful names and expose provenance', () => {
  const html = renderToStaticMarkup(
    <MostActivePanel
      actors={{
        agent: { active: 1, top: [] },
        device: { active: 0, top: [] },
        ip: { active: 0, top: [] },
        client: { active: 1, top: [{ id: 'client-row', filter_id: 'sub:123', label: 'Codex CLI', kind: 'client', calls: 3, detail: 'sub:123 · Self-reported MCP client' }] },
        subject: { active: 1, top: [] },
        unknown: { active: 1, top: [] },
      }}
      window="24h"
      onSelectActor={() => undefined}
    />,
  )

  assert.match(html, /Most active clients/)
  assert.match(html, /Self-reported MCP client/)
  for (const facet of ['Subjects', 'Clients', 'Agents', 'Unknown']) {
    assert.match(html, new RegExp(`>${facet}<`))
  }
  assert.doesNotMatch(html, />Devices</)
  assert.doesNotMatch(html, />IPs</)
})
