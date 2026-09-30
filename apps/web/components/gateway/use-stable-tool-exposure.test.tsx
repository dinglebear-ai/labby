import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Window } from 'happy-dom'

import type { DiscoveredTool } from '@/lib/types/gateway'
import { useStableToolExposure } from './use-stable-tool-exposure'

test('equivalent fresh detail tool arrays do not repeat the exposure reset effect', async () => {
  const window = new Window({ url: 'http://localhost/' })
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })

  const container = document.createElement('div')
  document.body.append(container)
  const root = createRoot(container)
  let effectRuns = 0

  function DetailExposureHarness({ tools }: { tools: DiscoveredTool[] }) {
    const { currentExposedToolNames } = useStableToolExposure(tools)
    const [draft, setDraft] = useState<string[]>([])

    useEffect(() => {
      effectRuns += 1
      setDraft(currentExposedToolNames)
    }, [currentExposedToolNames])

    return <span>{draft.join(',')}</span>
  }

  const tools: DiscoveredTool[] = [
    { name: 'alpha', description: 'first', exposed: true, matched_by: '*' },
    { name: 'beta', description: 'second', exposed: false, matched_by: null },
  ]

  await act(async () => {
    root.render(<DetailExposureHarness tools={tools} />)
  })
  await act(async () => {
    root.render(
      <DetailExposureHarness
        tools={tools.map((tool) => ({ ...tool, description: `${tool.description} refreshed` }))}
      />,
    )
  })

  assert.equal(effectRuns, 1)
  assert.equal(container.textContent, 'alpha')

  await act(async () => root.unmount())
  await window.happyDOM.abort()
})
