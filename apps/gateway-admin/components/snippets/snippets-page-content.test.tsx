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
      (candidate) => candidate.textContent?.trim() === 'New snippet',
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
    assert.match(document.body.textContent ?? '', /Use tool ids from the Tools catalog/)
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
