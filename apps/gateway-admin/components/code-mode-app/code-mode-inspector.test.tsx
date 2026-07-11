import assert from 'node:assert/strict'
import { test } from 'node:test'

import { act } from 'react'
import React from 'react'

import { CodeModeInspector } from './code-mode-inspector'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

test('renders execute call rows with redacted params', async () => {
  installTestDom()
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

  assert.match(container.textContent ?? '', /Execute calls/)
  assert.match(container.textContent ?? '', /github \/ search_issues/)
  assert.doesNotMatch(container.textContent ?? '', /\bok\b/)
  assert.ok(container.querySelector('[aria-label="success"]'))
  assert.match(container.textContent ?? '', /12ms/)
  assert.doesNotMatch(container.textContent ?? '', /\[redacted\]/)
  await act(async () => {
    const details = container.querySelector('details')
    assert.ok(details)
    details.open = true
    details.dispatchEvent(new window.Event('toggle', { bubbles: true }))
  })
  assert.match(container.textContent ?? '', /\[redacted\]/)
  assert.doesNotMatch(container.textContent ?? '', /raw-secret-token/)
  await unmount()
})

test('caps execute call rows for large traces', async () => {
  installTestDom()
  const calls = Array.from({ length: 75 }, (_, index) => ({
    id: `demo::tool_${index}`,
    namespace: 'demo',
    tool: `tool_${index}`,
    ok: true,
    elapsed_ms: index,
  }))
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: calls.length,
        calls,
      }}
    />,
  )

  assert.match(container.textContent ?? '', /50 shown/)
  assert.match(container.textContent ?? '', /25 hidden/)
  assert.match(container.textContent ?? '', /demo \/ tool_49/)
  assert.doesNotMatch(container.textContent ?? '', /demo \/ tool_50/)
  await unmount()
})

test('renders search match rows', async () => {
  installTestDom()
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

  assert.match(container.textContent ?? '', /Search matches/)
  assert.match(container.textContent ?? '', /axon \/ ask/)
  assert.match(container.textContent ?? '', /schema/)
  await unmount()
})

test('renders search truncation count metadata', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_search_trace',
        query_kind: 'catalog_filter',
        displayed_count: 50,
        truncated: true,
        match_count: 200,
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

  assert.match(container.textContent ?? '', /50 of 200/)
  assert.match(container.textContent ?? '', /truncated/)
  await unmount()
})

test('renders reduced search result shape and value when no tool rows match', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_search_trace',
        query_kind: 'catalog_filter',
        match_count: 0,
        matches: [],
        result_shape: { type: 'object', key_count: 2, keys: ['total', 'upstreams'] },
        result: { total: 398, upstreams: 42 },
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Search result/)
  assert.match(container.textContent ?? '', /reduced value/)
  assert.match(container.textContent ?? '', /keys: total, upstreams/)
  await act(async () => {
    const details = container.querySelector('details')
    assert.ok(details)
    details.open = true
    details.dispatchEvent(new window.Event('toggle', { bubbles: true }))
  })
  assert.match(container.textContent ?? '', /398/)
  await unmount()
})

test('renders history rows and flattened nested calls', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          {
            seq: 7,
            kind: 'execute',
            ok: 'false',
            elapsed_ms: 24,
            error_kind: 'tool_failed',
            calls: [
              {
                id: 'github::search_issues',
                upstream: 'github',
                tool: 'search_issues',
                ok: 'false',
                elapsed_ms: 12,
                error_kind: 'tool_failed',
              },
            ],
          },
        ],
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Recent history/)
  assert.match(container.textContent ?? '', /#7 execute/)
  assert.match(container.textContent ?? '', /tool_failed/)
  assert.match(container.textContent ?? '', /github \/ search_issues/)
  assert.doesNotMatch(container.textContent ?? '', /\bok\b/)
  await unmount()
})

test('renders parser warnings for dropped history rows', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          { seq: 1, kind: 'search', ok: true, elapsed_ms: 3 },
          { seq: 2, kind: 'unknown', ok: true, elapsed_ms: 3 },
        ],
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Dropped 1 malformed history entry/)
  await unmount()
})

test('updates from bridge tool results using both structured content field names', async () => {
  installTestDom()
  let instance:
    | {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        connect: () => Promise<unknown>
      }
    | undefined

  globalThis.window.ExtApps = {
    App: class {
      ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
      connect = async () => ({})
      constructor() {
        // eslint-disable-next-line @typescript-eslint/no-this-alias -- capturing the mock instance for later test assertions
        instance = this
      }
    },
  }

  const { container, unmount } = await renderClient(<CodeModeInspector />)

  await act(async () => {
    instance?.ontoolresult?.({
      structuredContent: {
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
      },
    })
  })

  assert.match(container.textContent ?? '', /axon \/ ask/)

  await act(async () => {
    instance?.ontoolresult?.({
      structured_content: {
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [
          {
            id: 'github::search_issues',
            upstream: 'github',
            tool: 'search_issues',
            ok: true,
            elapsed_ms: 12,
          },
        ],
      },
    })
  })

  assert.match(container.textContent ?? '', /github \/ search_issues/)
  await unmount()
})

test('renders a warning for malformed bridge payloads', async () => {
  installTestDom()
  let instance:
    | {
        ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
        connect: () => Promise<unknown>
      }
    | undefined

  globalThis.window.ExtApps = {
    App: class {
      ontoolresult?: (result: { structuredContent?: unknown; structured_content?: unknown }) => void
      connect = async () => ({})
      constructor() {
        // eslint-disable-next-line @typescript-eslint/no-this-alias -- capturing the mock instance for later test assertions
        instance = this
      }
    },
  }

  const { container, unmount } = await renderClient(<CodeModeInspector />)

  await act(async () => {
    instance?.ontoolresult?.({ structuredContent: { kind: 'tool_explorer' } })
  })

  assert.match(container.textContent ?? '', /Ignored malformed bridge payload/)
  await unmount()
})

test('renders empty state without bridge data', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(<CodeModeInspector />)

  assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)
  await unmount()
})

test('hydrates from window.openai.toolOutput on first paint', async () => {
  installTestDom()
  globalThis.window.openai = {
    toolOutput: {
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
    },
  }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    assert.match(container.textContent ?? '', /axon \/ ask/)
    await unmount()
  } finally {
    globalThis.window.openai = undefined
  }
})

test('updates from the openai:set_globals event detail.globals', async () => {
  installTestDom()
  globalThis.window.openai = { toolOutput: undefined }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    // Empty until the host pushes tool output.
    assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)

    await act(async () => {
      globalThis.window.dispatchEvent(
        new globalThis.window.CustomEvent('openai:set_globals', {
          detail: {
            globals: {
              toolOutput: {
                kind: 'code_mode_execute_trace',
                call_count: 1,
                calls: [
                  {
                    id: 'github::search_issues',
                    upstream: 'github',
                    tool: 'search_issues',
                    ok: true,
                    elapsed_ms: 12,
                  },
                ],
              },
            },
          },
        }),
      )
    })

    assert.match(container.textContent ?? '', /github \/ search_issues/)
    await unmount()
  } finally {
    globalThis.window.openai = undefined
  }
})

test('surfaces a warning for malformed window.openai payloads', async () => {
  installTestDom()
  globalThis.window.openai = { toolOutput: { kind: 'tool_explorer' } }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    assert.match(container.textContent ?? '', /Ignored malformed bridge payload/)
    await unmount()
  } finally {
    globalThis.window.openai = undefined
  }
})

test('clears the trace when openai:set_globals carries a null toolOutput', async () => {
  installTestDom()
  globalThis.window.openai = {
    toolOutput: {
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
    },
  }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    assert.match(container.textContent ?? '', /axon \/ ask/)

    await act(async () => {
      globalThis.window.dispatchEvent(
        new globalThis.window.CustomEvent('openai:set_globals', {
          detail: { globals: { toolOutput: null } },
        }),
      )
    })

    // Host cleared the result — the stale trace is dropped, not left as "connected".
    assert.doesNotMatch(container.textContent ?? '', /axon \/ ask/)
    assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)
    await unmount()
  } finally {
    globalThis.window.openai = undefined
  }
})
