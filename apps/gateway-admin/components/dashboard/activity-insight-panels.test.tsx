import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'

import { FanOutPanel, MostActivePanel } from './activity-insight-panels.tsx'

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
