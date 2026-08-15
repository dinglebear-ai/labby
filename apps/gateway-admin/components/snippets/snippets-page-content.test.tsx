import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { SidebarProvider } from '@/components/ui/sidebar'
import { SnippetsPageContent } from './snippets-page-content'

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
  }
  throw lastError
}

test('snippets page renders fetched snippets and typed inputs', async () => {
  installTestDom()
  const requests: Array<{ action?: string; params?: Record<string, unknown> }> = []
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string; params?: Record<string, unknown> }
    requests.push(payload)
    if (payload.action === 'snippets.list') {
      return new Response(JSON.stringify({
        snippets: [
          {
            name: 'homelab-readonly-pulse',
            description: 'Read-only homelab pulse',
            tags: ['homelab'],
            inputs: {
              host: {
                ty: 'string',
                required: false,
                default: 'node-a',
                description: 'Host alias',
              },
            },
            source: 'builtin',
            path: '/docs/snippets/homelab-readonly-pulse.md',
            shadowed: false,
          },
        ],
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'snippets.get') {
      return new Response(JSON.stringify({
        name: 'homelab-readonly-pulse',
        description: 'Read-only homelab pulse',
        tags: ['homelab'],
        inputs: {
          host: {
            ty: 'string',
            required: false,
            default: 'node-a',
            description: 'Host alias',
          },
        },
        source: 'builtin',
        path: '/docs/snippets/homelab-readonly-pulse.md',
        shadowed: false,
        body: [
          '---',
          'name: homelab-readonly-pulse',
          'description: Read-only homelab pulse',
          '---',
          '',
          '# Homelab Pulse',
          '',
          'Use **read-only** checks before changing anything.',
          '',
          '<script>alert("nope")</script>',
          '![tracking pixel](https://example.com/pixel.png)',
          '[bad link](javascript:alert("nope"))',
          '',
          '```js',
          'async () => ({ ok: true })',
          '```',
        ].join('\n'),
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'snippets.test') {
      return new Response(JSON.stringify({ name: 'homelab-readonly-pulse', passed: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ valid: true, name: 'homelab-readonly-pulse', mode: 'existing' }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
    </SidebarProvider>,
  )

  await waitFor(() => assert.match(view.container.textContent ?? '', /homelab-readonly-pulse/))
  assert.match(view.container.textContent ?? '', /Host alias/)

  // The mock renders the declared default as the input's placeholder, and the
  // typed value feeds test/exec params.
  const hostInput = view.container.querySelector<HTMLInputElement>('input[aria-label="host"]')
  assert.ok(hostInput, 'expected an input for the `host` snippet input')
  assert.equal(hostInput.getAttribute('placeholder'), 'node-a')

  // Mock table chrome: six columns, with the metric columns dashed out because
  // the snippets API exposes no run telemetry.
  const bodyText = view.container.textContent ?? ''
  for (const column of ['Snippet', 'Servers', 'Runs', 'Fails', 'Avg', 'History']) {
    assert.match(bodyText, new RegExp(column), `expected a ${column} column header`)
  }
  assert.match(bodyText, /1 of 1 snippets/)
  assert.equal(
    view.container.querySelectorAll('[title^="Runs — the snippets API"]').length,
    1,
    'expected the runs column to render a missing-value cell',
  )

  await waitFor(() => {
    const headings = Array.from(view.container.querySelectorAll('h1')).map((node) => node.textContent)
    assert.ok(headings.includes('Homelab Pulse'))
  })
  assert.deepEqual(requests.slice(0, 2).map((request) => request.action), ['snippets.list', 'snippets.get'])
  assert.deepEqual(requests[1]?.params, { name: 'homelab-readonly-pulse' })
  assert.equal(view.container.querySelector('script'), null)
  assert.equal(view.container.querySelector('img'), null)
  assert.equal(
    Array.from(view.container.querySelectorAll('a')).some((link) =>
      link.getAttribute('href')?.startsWith('javascript:'),
    ),
    false,
  )

  const testButton = Array.from(view.container.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === 'Test',
  )
  assert.ok(testButton, 'expected Test button')
  await act(async () => {
    testButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await waitFor(() => assert.match(view.container.textContent ?? '', /Test failed/))

  await view.unmount()
})

test('snippets table filters by tag pill and search, and dashes out absent metrics', async () => {
  installTestDom()
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    if (payload.action === 'snippets.list') {
      return new Response(JSON.stringify({
        snippets: [
          {
            name: 'alpha-pulse',
            description: 'Alpha check',
            tags: ['homelab'],
            source: 'builtin',
            path: '/docs/snippets/alpha-pulse.md',
            shadowed: false,
          },
          {
            name: 'beta-sweep',
            description: 'Beta sweep',
            tags: ['research'],
            source: 'user',
            path: '/home/u/.labby/snippets/beta-sweep.md',
            shadowed: false,
          },
        ],
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    return new Response(JSON.stringify({
      name: 'alpha-pulse',
      description: 'Alpha check',
      tags: ['homelab'],
      source: 'builtin',
      path: '/docs/snippets/alpha-pulse.md',
      shadowed: false,
      body: '---\nname: alpha-pulse\n---\n\n```js\nasync () => await callTool("dozzle::list_containers", {})\n```',
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
    </SidebarProvider>,
  )

  await waitFor(() => assert.match(view.container.textContent ?? '', /2 of 2 snippets/))
  assert.match(view.container.textContent ?? '', /beta-sweep/)

  // The selected snippet's resolved body is the only place upstream tools are
  // recoverable — the list payload has no servers field.
  await waitFor(() => assert.match(view.container.textContent ?? '', /dozzle::list_containers/))

  const researchPill = Array.from(view.container.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === 'research',
  )
  assert.ok(researchPill, 'expected a research tag pill')
  await act(async () => {
    researchPill.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await waitFor(() => assert.match(view.container.textContent ?? '', /1 of 2 snippets/))
  assert.doesNotMatch(view.container.textContent ?? '', /alpha-pulse/)

  await act(async () => {
    researchPill.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await waitFor(() => assert.match(view.container.textContent ?? '', /2 of 2 snippets/))

  const search = view.container.querySelector<HTMLInputElement>('input[aria-label="Search snippets"]')
  assert.ok(search, 'expected a search input')
  assert.equal(search.getAttribute('placeholder'), 'Search snippets, tools, tags…')

  await view.unmount()
})
