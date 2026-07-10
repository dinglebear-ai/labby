import assert from 'node:assert/strict'
import { test } from 'node:test'

import { act } from 'react'
import React from 'react'

import { CodeModeInspector } from './code-mode-inspector'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

async function clickButton(container: Element, matcher: (text: string) => boolean) {
  const button = [...container.querySelectorAll('button')].find((candidate) =>
    matcher(candidate.textContent?.trim() ?? ''),
  )
  assert.ok(button, 'expected a matching button')
  await act(async () => {
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await Promise.resolve()
  })
}

test('renders execute call rows with expandable redacted params', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [
          {
            id: 'github::search_issues',
            namespace: 'github',
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

  assert.match(container.textContent ?? '', /Execute/)
  assert.match(container.textContent ?? '', /1 call/)
  assert.match(container.textContent ?? '', /github/)
  assert.match(container.textContent ?? '', /search_issues/)
  assert.match(container.textContent ?? '', /12 ms/)
  assert.ok(container.querySelector('[aria-label="success"]'))
  // Params are collapsed until the call row is expanded.
  assert.doesNotMatch(container.textContent ?? '', /\[redacted\]/)
  await clickButton(container, (text) => text.includes('search_issues'))
  assert.match(container.textContent ?? '', /Redacted Params/)
  assert.match(container.textContent ?? '', /\[redacted\]/)
  assert.doesNotMatch(container.textContent ?? '', /raw-secret-token/)
  await unmount()
})

test('renders the result disclosure with shape summary and expandable value', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [
          { id: 'arcane::containers', namespace: 'arcane', tool: 'containers', ok: true, elapsed_ms: 96 },
        ],
        result: { containers: 24, notified: true },
        result_shape: { type: 'object', key_count: 2, size_bytes: 212, keys: ['containers', 'notified'] },
        input_tokens: 412,
        output_tokens: 96,
        logs_count: 2,
      }}
    />,
  )

  assert.match(container.textContent ?? '', /Result/)
  assert.match(container.textContent ?? '', /object · 2 keys · 212 B — keys: containers, notified/)
  // The result value is collapsed by default.
  assert.doesNotMatch(container.textContent ?? '', /"containers": 24/)
  await clickButton(container, (text) => text.startsWith('Result'))
  assert.match(container.textContent ?? '', /"containers": 24/)
  // Token/log metadata from the trace lands in the footer.
  assert.match(container.textContent ?? '', /412 in · 96 out/)
  assert.match(container.textContent ?? '', /2 logs/)
  await unmount()
})

test('renders in-sandbox search results as discovery match rows', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 0,
        calls: [],
        result: {
          results: [
            {
              id: 'unifi::device.list',
              namespace: 'unifi',
              name: 'device_list',
              description: 'List UniFi devices.',
            },
          ],
          total: 42,
          truncated: true,
        },
        result_shape: { type: 'object', key_count: 3 },
      }}
    />,
  )

  // Discovery runs make zero broker calls — the hits render instead of a
  // bare "No calls were made." line.
  assert.doesNotMatch(container.textContent ?? '', /No calls were made/)
  assert.match(container.textContent ?? '', /1 of 42 matches/)
  assert.match(container.textContent ?? '', /unifi/)
  assert.match(container.textContent ?? '', /device_list/)
  assert.match(container.textContent ?? '', /List UniFi devices/)
  await unmount()
})

test('renders the zero-match discovery hint', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 0,
        calls: [],
        result: { results: [], total: 0, truncated: false, hint: 'No matches. Broaden or try synonyms.' },
        result_shape: { type: 'object', key_count: 4 },
      }}
    />,
  )

  assert.match(container.textContent ?? '', /0 of 0 matches/)
  assert.match(container.textContent ?? '', /Broaden or try synonyms/)
  await unmount()
})

test('renders describe results as markdown behind the result row', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_execute_trace',
        call_count: 0,
        calls: [],
        result: {
          id: 'unifi::device.list',
          kind: 'tool',
          path: 'codemode.unifi.device_list',
          markdown: '# device_list\n\nList UniFi devices.',
        },
        result_shape: { type: 'object', key_count: 4 },
      }}
    />,
  )

  assert.match(container.textContent ?? '', /describe/)
  await clickButton(container, (text) => text.startsWith('Result'))
  assert.match(container.textContent ?? '', /# device_list/)
  await unmount()
})

test('selects the latest history entry and shows failure metadata', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          {
            seq: 6,
            kind: 'execute',
            ok: true,
            elapsed_ms: 921,
            calls: [{ id: 'gotify::message.create', ok: true, elapsed_ms: 903 }],
          },
          {
            seq: 7,
            kind: 'execute',
            ok: false,
            elapsed_ms: 1243,
            error_kind: 'upstream_timeout',
            calls: [
              {
                id: 'rustarr::qbittorrent.transfer_info',
                ok: false,
                elapsed_ms: 1010,
                error_kind: 'upstream_timeout',
                params: { instance: 'default' },
              },
            ],
          },
        ],
      }}
    />,
  )

  // The latest entry (#7, failed) is selected by default.
  assert.match(container.textContent ?? '', /upstream_timeout/)
  assert.match(container.textContent ?? '', /1.24 s/)
  // upstream/tool are derived from the history call id.
  assert.match(container.textContent ?? '', /rustarr/)
  assert.match(container.textContent ?? '', /qbittorrent.transfer_info/)
  assert.match(container.textContent ?? '', /Result not retained in history/)
  assert.match(container.textContent ?? '', /#6/)
  assert.match(container.textContent ?? '', /#7/)

  // Switching to #6 shows that entry's calls.
  await clickButton(container, (text) => text === '#6')
  assert.match(container.textContent ?? '', /message.create/)
  await unmount()
})

test('joins a live trace to its history entry for elapsed and chip labeling', async () => {
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

  const { container, unmount } = await renderClient(
    <CodeModeInspector
      initialTrace={{
        kind: 'code_mode_history',
        entries: [
          {
            seq: 9,
            kind: 'execute',
            ok: true,
            elapsed_ms: 348,
            execution_id: 'exec-9',
            calls: [{ id: 'arcane::containers', ok: true, elapsed_ms: 96 }],
          },
        ],
      }}
    />,
  )

  await act(async () => {
    instance?.ontoolresult?.({
      structuredContent: {
        kind: 'code_mode_execute_trace',
        call_count: 1,
        execution_id: 'exec-9',
        calls: [
          { id: 'arcane::containers', namespace: 'arcane', tool: 'containers', ok: true, elapsed_ms: 96 },
        ],
        result: { containers: 24 },
        result_shape: { type: 'object', key_count: 1 },
      },
    })
  })

  // Elapsed comes from the matching history entry; the chip is marked live.
  assert.match(container.textContent ?? '', /348 ms/)
  assert.match(container.textContent ?? '', /#9 live/)
  assert.match(container.textContent ?? '', /Result/)
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
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [{ id: 'axon::ask', namespace: 'axon', tool: 'ask', ok: true, elapsed_ms: 40 }],
      },
    })
  })

  assert.match(container.textContent ?? '', /ask/)

  await act(async () => {
    instance?.ontoolresult?.({
      structured_content: {
        kind: 'code_mode_execute_trace',
        call_count: 1,
        calls: [
          {
            id: 'github::search_issues',
            namespace: 'github',
            tool: 'search_issues',
            ok: true,
            elapsed_ms: 12,
          },
        ],
      },
    })
  })

  assert.match(container.textContent ?? '', /search_issues/)
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
      kind: 'code_mode_execute_trace',
      call_count: 1,
      calls: [{ id: 'axon::ask', namespace: 'axon', tool: 'ask', ok: true, elapsed_ms: 40 }],
    },
  }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    assert.match(container.textContent ?? '', /ask/)
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
                    namespace: 'github',
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

    assert.match(container.textContent ?? '', /search_issues/)
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
      kind: 'code_mode_execute_trace',
      call_count: 1,
      calls: [{ id: 'axon::ask', namespace: 'axon', tool: 'ask', ok: true, elapsed_ms: 40 }],
    },
  }
  try {
    const { container, unmount } = await renderClient(<CodeModeInspector />)
    assert.match(container.textContent ?? '', /ask/)

    await act(async () => {
      globalThis.window.dispatchEvent(
        new globalThis.window.CustomEvent('openai:set_globals', {
          detail: { globals: { toolOutput: null } },
        }),
      )
    })

    // Host cleared the result — the stale trace is dropped, not left as "connected".
    assert.doesNotMatch(container.textContent ?? '', /axon/)
    assert.match(container.textContent ?? '', /Waiting for an MCP Apps tool result/)
    await unmount()
  } finally {
    globalThis.window.openai = undefined
  }
})
