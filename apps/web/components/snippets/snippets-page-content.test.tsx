import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

// `SidebarProvider` and `SnippetsPageContent` are imported dynamically inside
// each test, after `installTestDom()` — not statically here. Both transitively
// pull in @radix-ui/react-use-layout-effect, which decides once at *module
// evaluation time* (`globalThis?.document ? React.useLayoutEffect : () => {}`)
// whether Radix's Portal-based components (Dialog, AlertDialog, ...) can ever
// mount. ES module imports are hoisted and evaluated before any code in this
// file's body runs, so a static top-level import here would permanently wire
// every Radix layout effect to a no-op in this test file's process — Portal's
// own `mounted` state would never flip, so it would render `null` forever and
// no dialog would ever appear in the DOM, regardless of `open` state. A
// dynamic `await import(...)` after `installTestDom()` makes `document` exist
// before Radix's module graph is ever evaluated. See bead lab-l9gpj.

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
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  // Removal reports through a sonner toast, which portals to document.body —
  // the same place these tests already look for the confirmation dialog.
  const { Toaster } = await import('@/components/ui/sonner')
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
          '[data link](data:text/html,unsafe)',
          '[vbscript link](vbscript:msgbox("unsafe"))',
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
      <Toaster />
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
  assert.deepEqual(requests.filter((request) => request.action !== 'snippets.history').slice(0, 2).map((request) => request.action), ['snippets.list', 'snippets.get'])
  assert.deepEqual(requests.find((request) => request.action === 'snippets.get')?.params, { name: 'homelab-readonly-pulse' })
  assert.equal(view.container.querySelector('script'), null)
  assert.equal(view.container.querySelector('img'), null)
  for (const link of view.container.querySelectorAll('a')) {
    const href = link.getAttribute('href') ?? ''
    const scheme = href.match(/^([a-z][a-z0-9+.-]*):/i)?.[1]?.toLowerCase()
    assert.ok(!scheme || ['http', 'https', 'mailto'].includes(scheme), `unsafe link: ${href}`)
  }

  const testButton = Array.from(view.container.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === 'Test live',
  )
  assert.ok(testButton, 'expected explicitly labeled live-test button')
  await act(async () => {
    testButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })
  await waitFor(() => assert.match(view.container.textContent ?? '', /Test failed/))
  assert.equal(requests.find((request) => request.action === 'snippets.test')?.params?.live, true)
  const offline = Array.from(view.container.querySelectorAll('button')).find((button) => button.textContent?.trim() === 'Test offline')
  assert.ok(offline)
  await act(async () => offline.dispatchEvent(new MouseEvent('click', { bubbles: true })))
  await waitFor(() => assert.equal(requests.filter((request) => request.action === 'snippets.test').length, 2))
  const offlineParams = requests.filter((request) => request.action === 'snippets.test')[1].params
  assert.equal(offlineParams?.live, undefined)
  assert.equal(offlineParams?.fixture, undefined)


  await view.unmount()
})

test('snippets table filters by tag pill and search, and dashes out absent metrics', async () => {
  installTestDom()
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  // Removal reports through a sonner toast, which portals to document.body —
  // the same place these tests already look for the confirmation dialog.
  const { Toaster } = await import('@/components/ui/sonner')
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
      <Toaster />
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

test('new snippet opens a guided intent-first builder with progressive disclosure', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'NodeFilter', { value: window.NodeFilter, configurable: true })
  Object.defineProperty(globalThis, 'HTMLInputElement', { value: window.HTMLInputElement, configurable: true })
  Object.defineProperty(globalThis, 'HTMLTextAreaElement', { value: window.HTMLTextAreaElement, configurable: true })
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    return new Response(JSON.stringify(payload.action === 'snippets.list' ? { snippets: [] } : {}), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const view = await renderClient(<SidebarProvider><SnippetsPageContent /></SidebarProvider>)
  try {
    const newSnippet = Array.from(view.container.querySelectorAll('button')).find(
      (candidate) => candidate.getAttribute('aria-label') === 'New snippet',
    )
    assert.ok(newSnippet)
    await act(async () => newSnippet.dispatchEvent(new MouseEvent('click', { bubbles: true })))
    await waitFor(() => assert.match(document.body.textContent ?? '', /Build a snippet/))
    assert.match(document.body.textContent ?? '', /Inspect one system/)
    assert.match(document.body.textContent ?? '', /Gather in parallel/)
    assert.doesNotMatch(document.body.textContent ?? '', /Selected tools/)

    const next = Array.from(document.body.querySelectorAll('button')).find(
      (candidate) => candidate.textContent?.includes('Use this pattern'),
    )
    assert.ok(next)
    await act(async () => next.dispatchEvent(new MouseEvent('click', { bubbles: true })))
    await waitFor(() => assert.match(document.body.textContent ?? '', /Selected tools/))
    assert.match(document.body.textContent ?? '', /Search the current authorized catalog/)
  } finally {
    await view.unmount()
  }
})

test('user snippets can be validated and overwritten from the editor', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'NodeFilter', { value: window.NodeFilter, configurable: true })
  Object.defineProperty(globalThis, 'HTMLInputElement', { value: window.HTMLInputElement, configurable: true })
  Object.defineProperty(globalThis, 'HTMLTextAreaElement', { value: window.HTMLTextAreaElement, configurable: true })
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  const requests: Array<{ action?: string; params?: Record<string, unknown> }> = []
  const snippet = {
    name: 'beta-sweep',
    description: 'Beta sweep',
    tags: ['research'],
    source: 'user',
    path: '/home/u/.labby/snippets/beta-sweep.md',
    shadowed: false,
    content_digest: 'snapshot-digest',
  }
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string; params?: Record<string, unknown> }
    requests.push(payload)
    if (payload.action === 'snippets.list') {
      return new Response(JSON.stringify({ snippets: [snippet] }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'snippets.get') {
      return new Response(JSON.stringify({
        ...snippet,
        body: 'async () => ({ ok: true })',
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    if (payload.action === 'snippets.validate') {
      return new Response(JSON.stringify({ valid: true, name: snippet.name, mode: 'body' }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify(snippet), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
    </SidebarProvider>,
  )
  try {
    await waitFor(() =>
      assert.ok(
        Array.from(view.container.querySelectorAll('button')).some(
          (candidate) => candidate.textContent?.trim() === 'Edit',
        ),
      ),
    )
    const editButton = Array.from(view.container.querySelectorAll('button')).find(
      (candidate) => candidate.textContent?.trim() === 'Edit',
    )
    assert.ok(editButton)
    await act(async () => {
      editButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    await waitFor(() => assert.match(document.body.textContent ?? '', /Edit beta-sweep/))

    const body = document.body.querySelector<HTMLTextAreaElement>('#snippet-edit-body')
    assert.ok(body)
    assert.equal(body.value, 'async () => ({ ok: true })')
    const save = Array.from(document.body.querySelectorAll('button')).find(
      (candidate) => candidate.textContent?.trim() === 'Validate and save',
    )
    assert.ok(save)
    await act(async () => {
      save.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    await waitFor(() =>
      assert.ok(requests.some((request) => request.action === 'snippets.create')),
    )
    const validateIndex = requests.findIndex((request) => request.action === 'snippets.validate')
    const createIndex = requests.findIndex((request) => request.action === 'snippets.create')
    assert.ok(validateIndex >= 0 && createIndex > validateIndex, 'draft must validate before overwrite')
    assert.deepEqual(requests[createIndex]?.params, {
      name: 'beta-sweep',
      body: 'async () => ({ ok: true })',
      description: 'Beta sweep',
      force: true,
      expected_digest: 'snapshot-digest',
    })
  } finally {
    await view.unmount()
  }
})

// bead lab-l9gpj
test('user snippets can be removed after confirmation; built-ins offer no Remove button', async () => {
  installTestDom()
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  // Removal reports through a sonner toast, which portals to document.body —
  // the same place these tests already look for the confirmation dialog.
  const { Toaster } = await import('@/components/ui/sonner')
  const requests: Array<{ action?: string; params?: Record<string, unknown> }> = []
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string; params?: Record<string, unknown> }
    requests.push(payload)
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
    if (payload.action === 'snippets.remove') {
      return new Response(JSON.stringify({ name: payload.params?.name, removed: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({
      name: 'alpha-pulse',
      description: 'Alpha check',
      tags: ['homelab'],
      source: 'builtin',
      path: '/docs/snippets/alpha-pulse.md',
      shadowed: false,
      body: '---\nname: alpha-pulse\n---\n\n```js\nasync () => ({ ok: true })\n```',
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
      <Toaster />
    </SidebarProvider>,
  )

  await waitFor(() => assert.match(view.container.textContent ?? '', /2 of 2 snippets/))

  // alpha-pulse (builtin) is selected by default and offers no Remove button.
  assert.equal(
    Array.from(view.container.querySelectorAll('button')).some((button) => button.textContent?.trim() === 'Remove'),
    false,
    'built-in snippets must not offer a Remove button',
  )

  const betaRow = Array.from(view.container.querySelectorAll('[role="button"]')).find((row) =>
    row.textContent?.includes('beta-sweep'),
  )
  assert.ok(betaRow, 'expected a beta-sweep row to select it')
  await act(async () => {
    betaRow.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })

  await waitFor(() =>
    assert.ok(
      Array.from(view.container.querySelectorAll('button')).some(
        (candidate) => candidate.textContent?.trim() === 'Remove',
      ),
      'expected a Remove button for the user-sourced snippet',
    ),
  )
  const removeButton = Array.from(view.container.querySelectorAll('button')).find(
    (candidate) => candidate.textContent?.trim() === 'Remove',
  )
  assert.ok(removeButton)
  await act(async () => {
    removeButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })

  await waitFor(() => assert.match(document.body.textContent ?? '', /Remove snippet\?/))
  assert.match(document.body.textContent ?? '', /beta-sweep/)

  const confirmButton = Array.from(document.body.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === 'Remove snippet',
  )
  assert.ok(confirmButton, 'expected a confirm button in the dialog')
  await act(async () => {
    confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })

  await waitFor(() =>
    assert.ok(
      requests.some((request) => request.action === 'snippets.remove' && request.params?.name === 'beta-sweep'),
      'expected a snippets.remove request for beta-sweep',
    ),
  )
  await waitFor(() => assert.doesNotMatch(document.body.textContent ?? '', /Remove snippet\?/))

  await view.unmount()
})

test('a failed snippet removal closes the dialog so the error is actually visible', async () => {
  // The error panel renders inside the snippet detail row, underneath this
  // dialog's modal overlay. Leaving the dialog open on failure hid the error
  // completely — the operator saw the button settle and nothing else change.
  installTestDom()
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  // Removal reports through a sonner toast, which portals to document.body —
  // the same place these tests already look for the confirmation dialog.
  const { Toaster } = await import('@/components/ui/sonner')
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    if (payload.action === 'snippets.list') {
      return new Response(JSON.stringify({
        snippets: [
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
    if (payload.action === 'snippets.remove') {
      return new Response(
        JSON.stringify({ kind: 'invalid_param', message: 'snippet `beta-sweep` is built in' }),
        { status: 400, headers: { 'content-type': 'application/json' } },
      )
    }
    return new Response(JSON.stringify({
      name: 'beta-sweep',
      description: 'Beta sweep',
      tags: ['research'],
      source: 'user',
      path: '/home/u/.labby/snippets/beta-sweep.md',
      shadowed: false,
      body: '---\nname: beta-sweep\n---\n\n```js\nasync () => ({ ok: true })\n```',
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
      <Toaster />
    </SidebarProvider>,
  )
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /beta-sweep/))

    const removeButton = await (async () => {
      await waitFor(() =>
        assert.ok(
          Array.from(view.container.querySelectorAll('button')).some(
            (candidate) => candidate.textContent?.trim() === 'Remove',
          ),
        ),
      )
      return Array.from(view.container.querySelectorAll('button')).find(
        (candidate) => candidate.textContent?.trim() === 'Remove',
      )
    })()
    assert.ok(removeButton)
    await act(async () => {
      removeButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    await waitFor(() => assert.match(document.body.textContent ?? '', /Remove snippet\?/))
    const confirmButton = Array.from(document.body.querySelectorAll('button')).find(
      (button) => button.textContent?.trim() === 'Remove snippet',
    )
    assert.ok(confirmButton)
    await act(async () => {
      confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    // The dialog must close, and the failure must be reported rather than
    // silently swallowed or shown as a success.
    await waitFor(() => assert.doesNotMatch(document.body.textContent ?? '', /Remove snippet\?/))
    await waitFor(() => assert.match(document.body.textContent ?? '', /Remove failed/))
    assert.doesNotMatch(document.body.textContent ?? '', /Removed beta-sweep/)
  } finally {
    await view.unmount()
  }
})

test('removing the last snippet still reports the success somewhere visible', async () => {
  installTestDom()
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  // Removal reports through a sonner toast, which portals to document.body —
  // the same place these tests already look for the confirmation dialog.
  const { Toaster } = await import('@/components/ui/sonner')

  // Removal destroys its own subject. Reporting through `actionState` — which
  // renders inside the *selected* snippet's detail row — meant the confirmation
  // landed under an unrelated snippet, or, when the removed one was the last,
  // nowhere at all: `reload` sets `selectedKey` to null, so no row exists to
  // host it and the operator saw the row vanish with no confirmation.
  let listed = false
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}')) as { action?: string }
    if (payload.action === 'snippets.list') {
      const snippets = listed
        ? []
        : [{
            name: 'only-one',
            description: 'The only snippet',
            tags: [],
            source: 'user',
            path: '/home/u/.labby/snippets/only-one.md',
            shadowed: false,
          }]
      listed = true
      return new Response(JSON.stringify({ snippets }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (payload.action === 'snippets.remove') {
      return new Response(JSON.stringify({ removed: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({
      name: 'only-one',
      description: 'The only snippet',
      tags: [],
      source: 'user',
      path: '/home/u/.labby/snippets/only-one.md',
      shadowed: false,
      body: '---\nname: only-one\n---\n\n```js\nasync () => ({ ok: true })\n```',
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  const view = await renderClient(
    <SidebarProvider>
      <SnippetsPageContent />
      <Toaster />
    </SidebarProvider>,
  )
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /only-one/))
    await waitFor(() =>
      assert.ok(
        Array.from(view.container.querySelectorAll('button')).some(
          (candidate) => candidate.textContent?.trim() === 'Remove',
        ),
      ),
    )
    const removeButton = Array.from(view.container.querySelectorAll('button')).find(
      (candidate) => candidate.textContent?.trim() === 'Remove',
    )
    assert.ok(removeButton)
    await act(async () => {
      removeButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    await waitFor(() => assert.match(document.body.textContent ?? '', /Remove snippet\?/))
    const confirmButton = Array.from(document.body.querySelectorAll('button')).find(
      (button) => button.textContent?.trim() === 'Remove snippet',
    )
    assert.ok(confirmButton)
    await act(async () => {
      confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    await waitFor(() => assert.doesNotMatch(document.body.textContent ?? '', /Remove snippet\?/))
    // The list is now empty — there is no detail row left to host a message,
    // so this can only pass because the toast lives outside the page content.
    await waitFor(() => assert.match(document.body.textContent ?? '', /Removed only-one/))
  } finally {
    await view.unmount()
  }
})

test('effective snippet rows hide shadowed builtins and surface invalid-file diagnostics', async () => {
  installTestDom()
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  const user = { name: 'shared', description: 'Effective user override', source: 'user', path: '/user/shared.md', shadowed: false, tags: [] }
  globalThis.fetch = (async (_input, init) => {
    const payload = JSON.parse(String(init?.body ?? '{}'))
    return new Response(JSON.stringify(payload.action === 'snippets.list' ? {
      snippets: [{ ...user, source: 'builtin', path: '/builtin/shared.md', shadowed: true }, user, { ...user, name: 'invalid', source: 'builtin', path: '/builtin/invalid.md', shadowed: true }],
      diagnostics_omitted: 7,
      diagnostics: [{ name: 'invalid', path: '/user/invalid.md', source: 'user', message: 'Invalid syntax' }],
    } : { ...user, body: 'async () => ({ ok: true })' }), { headers: { 'content-type': 'application/json' } })
  }) as typeof fetch
  const view = await renderClient(<SidebarProvider><SnippetsPageContent /></SidebarProvider>)
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /1 of 1 snippets/))
    assert.match(view.container.textContent ?? '', /overrides built-in/)
    assert.match(view.container.textContent ?? '', /7 additional diagnostics omitted/)
    assert.match(view.container.textContent ?? '', /\/user\/invalid.md: Invalid syntax/)
    assert.doesNotMatch(view.container.textContent ?? '', /\/builtin\/shared.md/)
  } finally { await view.unmount() }
})

test('fanout builder maps each tool independently and preserves batch statuses', async () => {
  const window = installTestDom()
  for (const name of ['NodeFilter', 'HTMLInputElement', 'HTMLTextAreaElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  globalThis.fetch = (async (url, init) => { const body=JSON.parse(String(init?.body??'{}'));const tool=String(body.query??'first::search');return new Response(JSON.stringify(String(url).includes('/tools/search')?{results:[{path:tool.replace('::','.'),id:tool,kind:'tool',namespace:tool.split('::')[0],name:tool.split('::')[1],description:'A tool',signature:'()',tags:[],score:1}],total:1,truncated:false}:String(url).includes('/tools/describe')?{path:body.target.replace('::','.'),id:body.target,namespace:'tool',name:'run',description:'Tool',helper:'tool.run',signature:'()',tags:[]}:{snippets:[]}),{headers:{'content-type':'application/json'}}) }) as typeof fetch
  const view = await renderClient(<SidebarProvider><SnippetsPageContent /></SidebarProvider>)
  const click = async (text: string) => {
    const button = Array.from(document.body.querySelectorAll('button')).find((entry) => entry.textContent?.includes(text))
    assert.ok(button, text)
    await act(async () => button.dispatchEvent(new MouseEvent('click', { bubbles: true })))
  }
  const type = async (selector: string, value: string) => {
    const field = document.body.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)
    assert.ok(field, selector)
    const prototype = field.tagName === 'INPUT' ? window.HTMLInputElement.prototype : window.HTMLTextAreaElement.prototype
    await act(async () => {
      Object.getOwnPropertyDescriptor(prototype, 'value')!.set!.call(field, value)
      // React's event module retains the first test window; invoke the current
      // controlled-field handler to avoid cross-window constructor mismatches.
      const propsKey = Object.keys(field).find((key) => key.startsWith('__reactProps$'))!
      const props = (field as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[propsKey]
      props.onChange({ target: { value } })
    })
  }
  try {
    const add = view.container.querySelector('button[aria-label="New snippet"]')!
    await act(async () => add.dispatchEvent(new MouseEvent('click', { bubbles: true })))
    await click('Gather in parallel')
    await click('Use this pattern')
    await type('#snippet-name', 'fanout-check')
    await type('#workflow-tool-search','first::search')
    await waitFor(()=>assert.match(document.body.textContent??'',/Add first::search/))
    await click('Add first::search')
    await type('#workflow-tool-search','second::status')
    await waitFor(()=>assert.match(document.body.textContent??'',/Add second::status/))
    await click('Add second::status')
    await type('#snippet-inputs', '{"bad key":"hello"}')
    await click('Build draft')
    assert.match(document.body.textContent ?? '', /Input name "bad key" must contain/)
    await type('#snippet-inputs', '{"query":"hello"}')
    await type('#tool-mapping-0', '{')
    await click('Build draft')
    assert.match(document.body.textContent ?? '', /JSON/)
    await type('#tool-mapping-0', '{"q":"$input.missing"}')
    await click('Build draft')
    assert.match(document.body.textContent ?? '', /Unknown snippet input missing/)
    await type('#tool-mapping-0', '{"q":"$input.toString"}')
    await click('Build draft')
    assert.match(document.body.textContent ?? '', /Unknown snippet input toString/)
    await type('#tool-mapping-0', '{"q":"$input.query"}')
    await type('#tool-mapping-1', '{"verbose":true,"__proto__":{"polluted":true}}')
    await click('Build draft')
    assert.match(document.body.textContent??'',/Unsafe mapping key/)
    await type('#tool-mapping-1','{"verbose":true}')
    await click('Build draft')
    const draft = document.body.textContent ?? ''
    assert.match(draft, /inputs:\s+query:/)
    assert.doesNotMatch(draft, /"query":\s+type:/)
    assert.match(draft, /await codemode.batch/)
    assert.match(draft, /first::search/)
    assert.match(draft, /second::status/)
    assert.doesNotMatch(draft, /Promise.all/)
    const body = Array.from(document.body.querySelectorAll('pre')).find(item=>item.textContent?.includes('```js'))!.textContent!
    const code = body.match(/```js\n([\s\S]*?)\n```/)![1]
    const calls: Array<{ tool: string; params: Record<string, unknown> }> = []
    const execute = new Function('input', 'callTool', 'codemode', `return (${code})`)(
      {query:'hello'},
      async (tool: string, params: Record<string, unknown>) => { calls.push({ tool, params }); return { ok: true } },
      { batch: async (jobs: Array<() => Promise<unknown>>) => { const ok=[];for(let i=0;i<jobs.length;i++)ok.push({i,value:await jobs[i]()});return {all_ok:true,ok,failed:[]} } },
    )
    await execute({ query: 'hello' })
    const statusParams = calls[1].params
    assert.equal(Object.getPrototypeOf(statusParams), Object.prototype)
    assert.deepEqual(statusParams,{verbose:true})
  } finally { await view.unmount() }
})

test('guided edits preserve required nullable typed contracts and optimistic digest without sample secrets',async()=>{
 const window=installTestDom();for(const name of ['NodeFilter','HTMLInputElement','HTMLTextAreaElement'] as const)Object.defineProperty(globalThis,name,{value:window[name],configurable:true})
 const {SidebarProvider}=await import('@/components/ui/sidebar');const {SnippetsPageContent}=await import('./snippets-page-content');const {generateWorkflowCode}=await import('./workflow-model')
 const inputs={ratio:{ty:'number',required:true,nullable:false,description:'Current ratio'},flag:{ty:'boolean',required:false,default:false},token:{ty:'string',required:true}}
 const snippet={name:'editable',source:'user',path:'/user/editable.md',description:'Editable workflow',tags:[],shadowed:false,content_digest:'source-digest',inputs,body:generateWorkflowCode({version:1,steps:[{id:'run',tool:'host::status',dependsOn:[],mapping:{ratio:'$input.ratio',token:'$input.token'}}]})}
 let created:Record<string,unknown>|undefined
 globalThis.fetch=(async(url,init)=>{const payload=JSON.parse(String(init?.body??'{}'));if(payload.action==='snippets.create')created=payload.params;return new Response(JSON.stringify(String(url).includes('/tools/describe')?{path:'host.status',id:'host::status',namespace:'host',name:'status',description:'Status',helper:'host.status',signature:'()',tags:[],input_schema:{type:'object',required:['ratio','token'],properties:{ratio:{type:'number'},token:{type:'string',writeOnly:true}}}}:payload.action==='snippets.list'?{snippets:[snippet]}:payload.action==='snippets.history'?{receipts:[],receipt_status:'persisted',next_cursor:null}:payload.action==='snippets.validate'?{valid:true}:snippet),{headers:{'content-type':'application/json'}})}) as typeof fetch
 const view=await renderClient(<SidebarProvider><SnippetsPageContent/></SidebarProvider>);const click=async(text:string)=>{const button=Array.from(document.body.querySelectorAll('button')).find(button=>button.textContent?.trim()===text);assert.ok(button,text);await act(async()=>button.click())}
 try{await waitFor(()=>assert.ok(Array.from(document.body.querySelectorAll('button')).some(button=>button.textContent==='Edit workflow')));await click('Edit workflow');await waitFor(()=>assert.ok(document.body.querySelector('[aria-label="Value source for host::status ratio"]')));await click('Build draft');await waitFor(()=>assert.match(document.body.textContent??'',/Runnable draft/));const body=Array.from(document.body.querySelectorAll('pre')).find(pre=>pre.textContent?.includes('```js'))!.textContent!;assert.match(body,/ratio:\n {4}type: number\n {4}required: true\n {4}nullable: false\n {4}description: Current ratio/);assert.match(body,/flag:\n {4}type: boolean\n {4}required: false\n {4}default: false/);assert.match(body,/token:\n {4}type: string\n {4}required: true/);await click('Validate and save');await waitFor(()=>assert.ok(created));assert.equal(created!.expected_digest,'source-digest');assert.equal(created!.force,true)}finally{await view.unmount()}
})

function reactChange(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const key = Object.keys(element).find(key => key.startsWith('__reactProps$'))!
  ;(element as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[key].onChange({ target: { value } })
}
function buttonNamed(name: string) {
  const button = Array.from(document.body.querySelectorAll('button')).find(button => button.textContent?.trim() === name || button.getAttribute('aria-label') === name)
  assert.ok(button, `Missing button ${name}`)
  return button
}
async function remediationPage(fetcher: typeof fetch) {
  const window = installTestDom()
  for (const name of ['NodeFilter', 'HTMLInputElement', 'HTMLTextAreaElement'] as const) Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  globalThis.fetch = fetcher
  const { SidebarProvider } = await import('@/components/ui/sidebar')
  const { SnippetsPageContent } = await import('./snippets-page-content')
  return renderClient(<SidebarProvider><SnippetsPageContent /></SidebarProvider>)
}
const jsonResponse = (value: unknown) => new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } })

test('guided edit waits for the newly selected workflow body', async () => {
  const { generateWorkflowCode } = await import('./workflow-model')
  const make = (name: string) => ({ name, source: 'user', path: `/${name}.md`, shadowed: false, tags: [], description: name, content_digest: name, body: `---\nname: ${name}\ndescription: ${name}\n---\n\n\`\`\`js\n${generateWorkflowCode({ version: 1, steps: [{ id: 'read', tool: 'host::read', mapping: {}, dependsOn: [] }] })}\n\`\`\`` })
  const a = make('alpha'), b = make('beta')
  let resolveB: (value: Response) => void = () => {}
  const view = await remediationPage((async (_input, init) => {
    const { action, params } = JSON.parse(String(init?.body))
    if (action === 'snippets.list') return jsonResponse({ snippets: [a, b] })
    if (action === 'snippets.get') return params.name === 'alpha' ? jsonResponse(a) : new Promise<Response>(resolve => { resolveB = resolve })
    return jsonResponse({ receipts: [] })
  }) as typeof fetch)
  try {
    await waitFor(() => assert.ok(buttonNamed('Edit workflow')))
    const row = Array.from(view.container.querySelectorAll('[role="button"]')).find(row => row.textContent?.includes('beta'))!
    await act(async () => row.dispatchEvent(new MouseEvent('click', { bubbles: true })))
    const edit = Array.from(document.body.querySelectorAll('button')).find(button => button.textContent?.trim() === 'Edit workflow')
    assert.ok(!edit || edit.disabled, 'old workflow must not remain editable under beta selection')
    await act(async () => resolveB(jsonResponse(b)))
    await waitFor(() => assert.equal(buttonNamed('Edit workflow').disabled, false))
    await act(async () => buttonNamed('Edit workflow').click())
    assert.equal(document.body.querySelector<HTMLInputElement>('#snippet-name')?.value, 'beta')
  } finally { await view.unmount() }
})

test('start from code saves a zero-tool custom snippet without guided generation', async () => {
  let saved: Record<string, unknown> | undefined
  const view = await remediationPage((async (_input, init) => {
    const { action, params } = JSON.parse(String(init?.body))
    if (action === 'snippets.list') return jsonResponse({ snippets: [{ name: 'existing', source: 'user', path: '/existing.md', tags: [], shadowed: false }] })
    if (action === 'snippets.create') { saved = params; return jsonResponse({ name: params.name, source: 'user' }) }
    return jsonResponse({ valid: true })
  }) as typeof fetch)
  try {
    await waitFor(() => assert.ok(buttonNamed('New snippet')))
    await act(async () => buttonNamed('New snippet').click())
    const custom = Array.from(document.body.querySelectorAll('button')).find(button => button.textContent?.includes('Start from code'))!
    await act(async () => custom.click())
    const next = Array.from(document.body.querySelectorAll('button')).find(button => button.textContent?.includes('Use this pattern'))!
    await act(async () => next.click())
    await act(async () => reactChange(document.body.querySelector<HTMLInputElement>('#snippet-name')!, 'custom-workflow'))
    await act(async () => reactChange(document.body.querySelector<HTMLInputElement>('#snippet-description')!, 'Custom workflow'))
    const continueButton = Array.from(document.body.querySelectorAll('button')).find(button => /Build draft|Review code/.test(button.textContent ?? ''))!
    await act(async () => continueButton.click())
    const body = document.body.querySelector<HTMLTextAreaElement>('#snippet-body')
    assert.ok(body, 'custom branch must expose source editor without requiring a tool')
    await act(async () => reactChange(body, 'async () => ({ ok: true, custom: 42 })'))
    await act(async () => buttonNamed('Validate and save').click())
    await waitFor(() => assert.ok(saved))
    assert.equal(saved?.body, 'async () => ({ ok: true, custom: 42 })')
    assert.equal(saved?.name, 'custom-workflow')
  } finally { await view.unmount() }
})

test('description edits round-trip authoritative frontmatter and preserve other metadata', async () => {
  const original = '---\nname: description-edit\ndescription: Original description\ntags: [kept]\ninputs:\n  count:\n    type: integer\n    default: 2\ntools: []\n---\n\n```js\nasync () => ({ok:true})\n```'
  let stored = original
  const info = () => ({ name: 'description-edit', description: stored.match(/^description: (.*)$/m)?.[1], source: 'user', path: '/description-edit.md', shadowed: false, tags: ['kept'], content_digest: 'snapshot', body: stored })
  const view = await remediationPage((async (_input, init) => {
    const { action, params } = JSON.parse(String(init?.body))
    if (action === 'snippets.list') return jsonResponse({ snippets: [info()] })
    if (action === 'snippets.get') return jsonResponse(info())
    if (action === 'snippets.create') { stored = params.body; return jsonResponse(info()) }
    if (action === 'snippets.history') return jsonResponse({ receipts: [] })
    return jsonResponse({ valid: true })
  }) as typeof fetch)
  try {
    await waitFor(() => assert.equal(buttonNamed('Edit').disabled, false))
    await act(async () => buttonNamed('Edit').click())
    await act(async () => reactChange(document.body.querySelector<HTMLInputElement>('#snippet-edit-description')!, 'Updated description'))
    await act(async () => buttonNamed('Validate and save').click())
    await waitFor(() => assert.match(stored, /^description: Updated description$/m))
    assert.equal(stored.replace('Updated description', 'Original description'), original)
    await act(async () => buttonNamed('Edit').click())
    assert.equal(document.body.querySelector<HTMLInputElement>('#snippet-edit-description')!.value, 'Updated description')
    // Source stays authoritative when the separate description control is untouched.
    await act(async () => reactChange(document.body.querySelector<HTMLTextAreaElement>('#snippet-edit-body')!, stored.replace('Updated description', 'Source description')))
    await act(async () => buttonNamed('Validate and save').click())
    await waitFor(() => assert.match(stored, /^description: Source description$/m))
  } finally { await view.unmount() }
})

test('guided save drops schema-sensitive aliased defaults while preserving ephemeral example values', async () => {
  const { generateWorkflowCode } = await import('./workflow-model')
  const plan = { version: 1 as const, steps: [{ id: 'connect', tool: 'host::connect', mapping: { config: '$input.settings' }, dependsOn: [] }] }
  const snippet = { name: 'sensitive-workflow', source: 'user', path: '/sensitive-workflow.md', shadowed: false, tags: [], description: 'Connect', content_digest: 'saved-digest', inputs: { settings: { ty: 'json', default: { value: 'private-fixture-value', host: 'node' }, required: false } }, body: `---\nname: sensitive-workflow\ndescription: Connect\n---\n\n\`\`\`js\n${generateWorkflowCode(plan)}\n\`\`\`` }
  let saved: Record<string, unknown> | undefined
  const view = await remediationPage((async (input, init) => {
    if (String(input).includes('/tools/describe')) return jsonResponse({ path: 'host.connect', id: 'host::connect', namespace: 'host', name: 'connect', description: 'Connect', helper: 'host.connect', signature: '()', tags: [], input_schema: { type: 'object', properties: { config: { type: 'object', properties: { value: { type: 'string', writeOnly: true }, host: { type: 'string' } } } } } })
    const { action, params } = JSON.parse(String(init?.body))
    if (action === 'snippets.list') return jsonResponse({ snippets: [snippet] })
    if (action === 'snippets.get') return jsonResponse(snippet)
    if (action === 'snippets.create') { saved = params; return jsonResponse(snippet) }
    if (action === 'snippets.history') return jsonResponse({ receipts: [] })
    return jsonResponse({ valid: true })
  }) as typeof fetch)
  try {
    await waitFor(() => assert.ok(buttonNamed('Edit workflow')))
    await act(async () => buttonNamed('Edit workflow').click())
    await waitFor(() => assert.match(document.body.textContent ?? '', /Parameters from the current tool schema/))
    assert.match(document.body.querySelector<HTMLTextAreaElement>('#snippet-inputs')!.value, /private-fixture-value/)
    await act(async () => buttonNamed('Build draft').click())
    await act(async () => buttonNamed('Validate and save').click())
    await waitFor(() => assert.ok(saved))
    assert.doesNotMatch(String(saved?.body), /private-fixture-value/)
    assert.match(String(saved?.body), /settings:\n {4}type: json\n {4}required: true/)
    assert.equal(saved?.expected_digest, 'saved-digest')
  } finally { await view.unmount() }
})
