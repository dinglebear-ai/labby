import assert from 'node:assert/strict'
import { test } from 'node:test'

import React from 'react'

import { CodeModeInspector } from './code-mode-inspector'
import { installChatTestDom, renderClient } from '@/components/chat/test-utils'

test('renders execute call rows with redacted params', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [
          {
            id: 'github::search_issues',
            upstream: 'github',
            tool: 'search_issues',
            ok: true,
            elapsed_ms: 12,
            params: { query: 'bug', token: '[redacted]' },
          },
        ],
        result_shape: { type: 'object', key_count: 2 },
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Broker-observed execute calls/)
  assert.match(container.textContent ?? '', /github \/ search_issues/)
  assert.match(container.textContent ?? '', /12ms/)
  assert.match(container.textContent ?? '', /\[redacted\]/)
  assert.doesNotMatch(container.textContent ?? '', /raw-secret-token/)
  await unmount()
})

test('renders search match rows', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_search_trace',
        query_kind: 'catalog_filter',
        match_count: 1,
        matches: [
          {
            id: 'axon::ask',
            upstream: 'axon',
            tool: 'ask',
            description: 'Ask indexed docs',
            has_schema: true,
            has_output_schema: false,
          },
        ],
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Catalog-inferred search matches/)
  assert.match(container.textContent ?? '', /axon \/ ask/)
  assert.match(container.textContent ?? '', /schema/)
  await unmount()
})

test('renders empty state without bridge data', async () => {
  installChatTestDom()
  const { container, unmount } = await renderClient(<CodeModeInspector />)

  assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)
  await unmount()
})
