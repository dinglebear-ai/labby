import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { SWRConfig } from 'swr'

import { gatewayApi } from '@/lib/api/gateway-client'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { CodeModeConfig, CodeModeConfigInput } from '@/lib/types/gateway'
import { CodeModeSearchSettings } from './code-mode-search-settings'

const initialConfig: CodeModeConfig = {
  enabled: true,
  timeout_ms: 5000,
  max_tool_calls: 8,
  max_response_bytes: 24 * 1024,
  max_response_tokens: 6000,
  search: {
    sources: ['personal_labby', 'team_depot', 'public_depot'],
    kinds: ['tool', 'skill', 'command', 'prompt', 'subagent', 'snippet'],
  },
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 20))
      })
    }
  }
  throw lastError
}

async function click(element: Element | undefined | null) {
  assert.ok(element)
  await act(async () => {
    ;(element as HTMLElement).click()
    await Promise.resolve()
  })
}

function findButton(container: Element, label: string): HTMLButtonElement | undefined {
  return [...container.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === label)
}

test('Code Mode search settings load all sources and kinds and save the selected policy', async () => {
  installTestDom()
  const originalGet = gatewayApi.getCodeModeConfig
  const originalSet = gatewayApi.setCodeModeConfig
  let received: CodeModeConfigInput | undefined
  gatewayApi.getCodeModeConfig = async () => initialConfig
  gatewayApi.setCodeModeConfig = async (input) => {
    received = input
    return {
      ...initialConfig,
      search: {
        sources: input.search_sources ?? initialConfig.search.sources,
        kinds: input.search_kinds ?? initialConfig.search.kinds,
      },
    }
  }

  try {
    const view = await renderClient(
      <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
        <CodeModeSearchSettings />
      </SWRConfig>,
    )
    await waitFor(() => assert.equal(view.container.querySelectorAll('[role="switch"]').length, 9))

    const labels = [...view.container.querySelectorAll('[role="switch"]')]
      .map((element) => element.getAttribute('aria-label'))
    assert.deepEqual(labels, [
      'Search Public Depot',
      'Search Team Depot',
      'Search Personal Labby',
      'Search Tools',
      'Search Skills',
      'Search Commands',
      'Search Prompts',
      'Search Subagents',
      'Search Snippets',
    ])

    await click(view.container.querySelector('[aria-label="Search Public Depot"]'))
    await click(view.container.querySelector('[aria-label="Search Skills"]'))
    assert.match(view.container.textContent ?? '', /Unsaved changes/)

    const unload = new Event('beforeunload', { cancelable: true })
    window.dispatchEvent(unload)
    assert.equal(unload.defaultPrevented, true)

    await click(findButton(view.container, 'Save'))
    await waitFor(() => assert.ok(received))
    assert.deepEqual(received?.search_sources, ['team_depot', 'personal_labby'])
    assert.deepEqual(received?.search_kinds, ['tool', 'command', 'prompt', 'subagent', 'snippet'])
    await waitFor(() => assert.match(view.container.textContent ?? '', /Saved settings apply/))
    assert.equal(view.container.querySelector('[aria-label="Search Public Depot"]')?.getAttribute('aria-checked'), 'false')
    assert.equal(view.container.querySelector('[aria-label="Search Skills"]')?.getAttribute('aria-checked'), 'false')

    await view.unmount()
  } finally {
    gatewayApi.getCodeModeConfig = originalGet
    gatewayApi.setCodeModeConfig = originalSet
  }
})

test('Code Mode search settings surface load failures and disable saving', async () => {
  installTestDom()
  const originalGet = gatewayApi.getCodeModeConfig
  gatewayApi.getCodeModeConfig = async () => {
    throw new Error('catalog policy denied')
  }

  try {
    const view = await renderClient(
      <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false }}>
        <CodeModeSearchSettings />
      </SWRConfig>,
    )
    await waitFor(() => assert.match(view.container.querySelector('[role="alert"]')?.textContent ?? '', /catalog policy denied/))
    assert.equal(findButton(view.container, 'Save')?.disabled, true)
    await view.unmount()
  } finally {
    gatewayApi.getCodeModeConfig = originalGet
  }
})
