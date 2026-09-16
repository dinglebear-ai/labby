import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom } from '../../lib/testing/dom-install.ts'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
installTestDom()
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })

test('global library tray links real routes and distinguishes zero from unavailable', async () => {
  const { ConsoleLibraryTray } = await import('./console-global-tools.tsx')
  const html = renderToStaticMarkup(<ConsoleLibraryTray counts={{ artifacts: 0, tools: 17 }} />)
  for (const path of ['/library', '/loadouts', '/snippets', '/tools']) assert.match(html, new RegExp(`href="${path}"`))
  assert.match(html, />0<\/span>/)
  assert.match(html, />17<\/span>/)
  assert.equal((html.match(/Count unavailable for the current authority/g) ?? []).length, 2)
})

test('Phoenix opens an explicit unavailable session panel without simulated send actions', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    const trigger = view.container.querySelector('button[aria-label="Ask Phoenix"]'); assert.ok(trigger)
    await act(async () => { trigger.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event) })
    const panel = document.querySelector('[aria-label="Phoenix session"]'); assert.ok(panel)
    assert.match(panel.textContent ?? '', /Session execution is unavailable/)
    assert.equal(panel.querySelector('textarea'), null)
    assert.equal(panel.querySelector('button[aria-label="Send"]'), null)
    assert.equal(panel.querySelector('a')?.getAttribute('href'), '/agents')
    const close = panel.querySelector('button[aria-label="Close Phoenix panel"]'); assert.ok(close)
    await act(async () => { close.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event) })
    assert.equal(document.querySelector('[aria-label="Phoenix session"]'), null)
  } finally { await view.unmount() }
})

test('Phoenix uses one control to dock right and return to a draggable resizable window', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const [{ PhoenixAvailability }, { ConsoleShellProvider, useConsoleShell }, { renderClient }] = await Promise.all([
    import('./console-global-tools.tsx'),
    import('./console-shell-context.tsx'),
    import('../../lib/testing/dom-test-utils.tsx'),
  ])
  function DockState() {
    const { phoenixDocked } = useConsoleShell()
    return <output data-phoenix-reserved-space={phoenixDocked ? 'right' : 'none'} />
  }
  const view = await renderClient(<ConsoleShellProvider><PhoenixAvailability/><DockState/></ConsoleShellProvider>)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    const panel = document.querySelector<HTMLElement>('[aria-label="Phoenix session"]'); assert.ok(panel)
    assert.equal(panel.getAttribute('data-dock'), 'float')
    assert.ok(panel.querySelector('button[aria-label="Resize Phoenix panel"]'))
    assert.equal(panel.querySelectorAll('button[aria-label="Dock Phoenix right"]').length, 1)
    assert.equal(panel.querySelector('button[aria-label="Dock Phoenix left"]'), null)
    const resize = panel.querySelector<HTMLButtonElement>('button[aria-label="Resize Phoenix panel"]'); assert.ok(resize)
    const originalWidth = Number.parseFloat(panel.style.width)
    await act(async () => resize.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, clientX: 500, clientY: 500 })))
    await act(async () => { window.dispatchEvent(new window.PointerEvent('pointermove', { bubbles: true, clientX: 460, clientY: 460 })); await new Promise((resolve) => setTimeout(resolve, 20)) })
    assert.equal(Number.parseFloat(panel.style.width), originalWidth - 40)
    window.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true }))

    const header = panel.querySelector<HTMLElement>('[data-phoenix-drag-handle]'); assert.ok(header)
    const originalLeft = Number.parseFloat(panel.style.left)
    await act(async () => header.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, clientX: 20, clientY: 30 })))
    await act(async () => { window.dispatchEvent(new window.PointerEvent('pointermove', { bubbles: true, clientX: -15, clientY: 70 })); await new Promise((resolve) => setTimeout(resolve, 20)) })
    assert.equal(Number.parseFloat(panel.style.left), originalLeft - 35)

    await act(async () => panel.querySelector<HTMLButtonElement>('button[aria-label="Dock Phoenix right"]')!.click())
    assert.equal(panel.getAttribute('data-dock'), 'right')
    assert.equal(view.container.querySelector('output')?.getAttribute('data-phoenix-reserved-space'), 'right')
    const float = panel.querySelector<HTMLButtonElement>('button[aria-label="Float Phoenix panel"]'); assert.ok(float)
    assert.equal(panel.querySelectorAll('button[aria-label="Float Phoenix panel"]').length, 1)

    await act(async () => float.click())
    assert.equal(panel.getAttribute('data-dock'), 'float')
    assert.equal(view.container.querySelector('output')?.getAttribute('data-phoenix-reserved-space'), 'none')
  } finally { await view.unmount() }
})

test('Phoenix sends a real turn through the container-local service and renders the reply', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    actions.push(body.action)
    if (body.action === 'phoenix.status') return new Response(JSON.stringify({ enabled: true, available: true, runtime: 'container_local', service: 'codex-app-server', sandbox: 'read-only' }), { status: 200 })
    if (body.action === 'phoenix.models.list') return new Response(JSON.stringify({ models: [{ id: 'gpt', model: 'gpt', displayName: 'GPT', description: 'Fast model', isDefault: true, defaultReasoningEffort: 'medium', supportedReasoningEfforts: [{ reasoningEffort: 'medium', description: 'Balanced reasoning' }] }] }), { status: 200 })
    if (body.action === 'phoenix.session.list') return new Response(JSON.stringify({ sessions: [] }), { status: 200 })
    if (body.action === 'phoenix.session.start') return new Response(JSON.stringify({ session_id: 'phoenix-1', status: 'ready', messages: [] }), { status: 200 })
    return new Response(JSON.stringify({ session_id: 'phoenix-1', status: 'ready', messages: [{ role: 'user', text: 'Is Labby healthy?' }, { role: 'assistant', text: 'The gateway is healthy.' }], events: [{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 4096 }, modelContextWindow: 200_000 } } }] }), { status: 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const panel = document.querySelector('[aria-label="Phoenix session"]'); assert.ok(panel)
    assert.match(panel.textContent ?? '', /Codexlabby/)
    assert.match(panel.textContent ?? '', /Attached to the container-local session/)
    assert.equal(panel.querySelectorAll('button').length >= 5, true)
    assert.ok(panel.querySelector('button[aria-label="Switch Phoenix thread"]'))
    assert.ok(panel.querySelector('button[aria-label="Phoenix settings"]'))
    assert.equal(panel.querySelector('select'), null)
    assert.equal(panel.querySelector('button[aria-label="Choose Phoenix model"]')?.textContent, 'AI')
    assert.ok(panel.querySelector('button[aria-label="Choose Phoenix reasoning"]'))
    const dockRight = panel.querySelector<HTMLButtonElement>('button[aria-label="Dock Phoenix right"]'); assert.ok(dockRight)
    await act(async () => dockRight.click())
    assert.equal(panel.getAttribute('data-dock'), 'right')
    const floatPanel = panel.querySelector<HTMLButtonElement>('button[aria-label="Float Phoenix panel"]'); assert.ok(floatPanel)
    await act(async () => floatPanel.click())
    assert.equal(panel.getAttribute('data-dock'), 'float')
    const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]')
    assert.ok(input)
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set
      setter?.call(input, 'Is Labby healthy?')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'Is Labby healthy?' }) as unknown as Event)
    })
    await act(async () => input.form!.requestSubmit())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)) })
    assert.deepEqual(actions.filter((action) => action !== 'phoenix.session.list'), ['phoenix.status', 'phoenix.models.list', 'phoenix.session.start', 'phoenix.turn.send'])
    assert.match(panel.textContent ?? '', /The gateway is healthy/)
    assert.equal(panel.querySelectorAll('[data-phoenix-message]').length, 2)
    assert.ok(panel.querySelector('[aria-label="Context usage 2% (4,096 / 200,000 tokens)"]'))
    assert.ok(panel.querySelector('button[aria-label="Edit message"]'))
    assert.ok(panel.querySelector('button[aria-label="Regenerate"]'))
    assert.ok(panel.querySelector('button[aria-label="Copy answer"]'))
    assert.equal(document.querySelector('[aria-label="Close Phoenix panel"]')?.classList.contains('size-11'), true)
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix exposes a Stop control that interrupts an in-flight turn', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  let resolveSend: ((response: Response) => void) | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    actions.push(body.action)
    if (body.action === 'phoenix.status') return new Response(JSON.stringify({ enabled: true, available: true, runtime: 'container_local', service: 'codex-app-server', sandbox: 'read-only', capabilities: { turn_lifecycle: ['start', 'completed', 'interrupt'] } }), { status: 200 })
    if (body.action === 'phoenix.models.list') return new Response(JSON.stringify({ models: [] }), { status: 200 })
    if (body.action === 'phoenix.session.list') return new Response(JSON.stringify({ sessions: [] }), { status: 200 })
    if (body.action === 'phoenix.session.start') return new Response(JSON.stringify({ session_id: 'phoenix-stop', status: 'ready', messages: [] }), { status: 200 })
    if (body.action === 'phoenix.turn.interrupt') return new Response(JSON.stringify({ session_id: 'phoenix-stop', status: 'ready', messages: [{ role: 'user', text: 'Keep working' }], events: [{ method: 'turn/interrupted', params: {} }] }), { status: 200 })
    return new Promise<Response>((resolve) => { resolveSend = resolve })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]'); assert.ok(input)
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set?.call(input, 'Keep working')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'Keep working' }) as unknown as Event)
    })
    await act(async () => { input.form!.requestSubmit(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    const stop = document.querySelector<HTMLButtonElement>('button[aria-label="Stop Phoenix"]'); assert.ok(stop)
    await act(async () => { stop.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.deepEqual(actions.filter((action) => action !== 'phoenix.session.list'), ['phoenix.status', 'phoenix.models.list', 'phoenix.session.start', 'phoenix.turn.send', 'phoenix.turn.interrupt'])
    resolveSend?.(new Response(JSON.stringify({ session_id: 'phoenix-stop', status: 'ready', messages: [{ role: 'user', text: 'Keep working' }, { role: 'assistant', text: 'Stopped.' }] }), { status: 200 }))
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix clears prior thread metadata immediately when browser identity changes', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-a' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-a', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'operator-a', organizationId: 'org', activeOwner: { kind: 'personal', id: 'operator-a' }, teams: [], projects: [], capabilities: ['scope.read'], generation: 1 } })
  const originalFetch = globalThis.fetch
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  let statusCalls = 0
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'phoenix.status') {
      statusCalls += 1
      if (statusCalls > 1) return new Promise<Response>(() => undefined)
      return response({ enabled: true, available: true })
    }
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [{ session_id: 'secret-thread', title: 'PRIOR_USER_SECRET', preview: 'private preview', message_count: 2, turn_status: 'ready' }] })
    return response({})
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Switch Phoenix thread"]')!.click())
    assert.match(document.body.textContent ?? '', /PRIOR_USER_SECRET/)

    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-b' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-b', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'operator-b', organizationId: 'org', activeOwner: { kind: 'personal', id: 'operator-b' }, teams: [], projects: [], capabilities: ['scope.read'], generation: 1 } })
    await act(async () => { await view.rerender(<PhoenixAvailability />); await new Promise((resolve) => setTimeout(resolve, 0)) })

    assert.doesNotMatch(document.body.textContent ?? '', /PRIOR_USER_SECRET/, 'prior identity thread metadata must be cleared before the next authority fetch completes')
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix ignores delayed diagnostics after an authority switch', async () => {
  const authority = (principalId: string) => ({ schemaVersion: 1 as const, compatibilityGeneration: 1 as const, principalId, organizationId: 'org', activeOwner: { kind: 'personal' as const, id: principalId }, teams: [], projects: [], capabilities: ['scope.read'], generation: 1 })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-a' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-a', authority: authority('operator-a') })
  const originalFetch = globalThis.fetch
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  let finishDiagnostics: ((response: Response) => void) | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true, capabilities: { diagnostics: ['read'] } })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [] })
    if (body.action === 'phoenix.diagnostics.read') return new Promise<Response>((resolve) => { finishDiagnostics = resolve })
    return response({})
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Phoenix settings"]')!.click())
    const diagnostics = document.querySelector<HTMLButtonElement>('button[aria-label="Read Phoenix diagnostics"]'); assert.ok(diagnostics)
    await act(async () => { diagnostics.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.ok(finishDiagnostics, 'diagnostics request is pending')

    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-b' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-b', authority: authority('operator-b') })
    await act(async () => { await view.rerender(<PhoenixAvailability />); await new Promise((resolve) => setTimeout(resolve, 0)) })
    await act(async () => { finishDiagnostics?.(response({ old_secret_marker: true })); await new Promise((resolve) => setTimeout(resolve, 0)) })

    assert.doesNotMatch(document.body.textContent ?? '', /old secret marker/, 'diagnostics from the prior authority must not update the new authority session')
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix does not carry a delayed attachment read across an authority switch', async () => {
  const authority = (principalId: string) => ({ schemaVersion: 1 as const, compatibilityGeneration: 1 as const, principalId, organizationId: 'org', activeOwner: { kind: 'personal' as const, id: principalId }, teams: [], projects: [], capabilities: ['scope.read'], generation: 1 })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-a' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-a', authority: authority('operator-a') })
  const originalFetch = globalThis.fetch
  const originalFileReader = globalThis.FileReader
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  const pendingReaders: Array<{ result: string | ArrayBuffer | null; onload: (() => void) | null }> = []
  class DeferredFileReader {
    result: string | ArrayBuffer | null = null
    onload: (() => void) | null = null
    onerror: (() => void) | null = null
    readAsDataURL() { pendingReaders.push(this) }
  }
  Object.defineProperty(globalThis, 'FileReader', { configurable: true, value: DeferredFileReader })
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [] })
    return response({})
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const fileInput = document.querySelector<HTMLInputElement>('input[aria-label="Attach image or file"]'); assert.ok(fileInput)
    Object.defineProperty(fileInput, 'files', { configurable: true, value: [new File(['secret'], 'prior-secret.txt', { type: 'text/plain' })] })
    await act(async () => fileInput.dispatchEvent(new window.Event('change', { bubbles: true })))
    const pendingReader = pendingReaders[0]
    assert.ok(pendingReader, 'attachment encoding is pending')

    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-b' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-b', authority: authority('operator-b') })
    await act(async () => { await view.rerender(<PhoenixAvailability />); await new Promise((resolve) => setTimeout(resolve, 0)) })
    pendingReader.result = 'data:text/plain;base64,c2VjcmV0'
    await act(async () => { pendingReader.onload?.(); await new Promise((resolve) => setTimeout(resolve, 0)) })

    assert.doesNotMatch(document.body.textContent ?? '', /prior-secret.txt/, 'a file selected by the prior authority must not appear in the new authority session')
  } finally {
    globalThis.fetch = originalFetch
    Object.defineProperty(globalThis, 'FileReader', { configurable: true, value: originalFileReader })
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix does not let a delayed close wipe a newly selected thread', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  let finishClose: ((response: Response) => void) | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: { session_id?: string } }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [
      { session_id: 'thread-a', title: 'Thread A', preview: 'A', message_count: 1, turn_status: 'ready' },
      { session_id: 'thread-b', title: 'Thread B', preview: 'B', message_count: 1, turn_status: 'ready' },
    ] })
    if (body.action === 'phoenix.session.read') {
      const id = body.params?.session_id
      return response({ session_id: id, status: 'ready', messages: [{ role: 'assistant', text: id === 'thread-a' ? 'A MESSAGE' : 'B MESSAGE' }], events: [] })
    }
    if (body.action === 'phoenix.session.close') return new Promise<Response>((resolve) => { finishClose = resolve })
    return response({})
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const threadsButton = () => document.querySelector<HTMLButtonElement>('button[aria-label="Switch Phoenix thread"]')!
    await act(async () => threadsButton().click())
    let menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const threadA = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('Thread A')); assert.ok(threadA)
    await act(async () => { threadA.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.match(document.body.textContent ?? '', /A MESSAGE/)

    await act(async () => threadsButton().click())
    menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const closeA = menu.querySelector<HTMLButtonElement>('button[aria-label="Close Thread A"]'); assert.ok(closeA)
    await act(async () => { closeA.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.ok(finishClose, 'close request is pending')
    const threadB = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('Thread B')); assert.ok(threadB)
    await act(async () => { threadB.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.match(document.body.textContent ?? '', /B MESSAGE/)

    await act(async () => { finishClose?.(response({ session_id: 'thread-a', status: 'closed' })); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.match(document.body.textContent ?? '', /B MESSAGE/, 'late close completion for Thread A must not clear the newly selected Thread B')
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix does not let a delayed rename failure overwrite a newly selected thread title', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
  let finishRename: ((response: Response) => void) | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: { session_id?: string; title?: string } }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [
      { session_id: 'thread-a', title: 'Thread A', preview: 'A', message_count: 1, turn_status: 'ready' },
      { session_id: 'thread-b', title: 'Thread B', preview: 'B', message_count: 1, turn_status: 'ready' },
    ] })
    if (body.action === 'phoenix.session.read') {
      const id = body.params?.session_id
      return response({ session_id: id, status: 'ready', messages: [{ role: 'assistant', text: id === 'thread-a' ? 'A MESSAGE' : 'B MESSAGE' }], events: [] })
    }
    if (body.action === 'phoenix.session.rename') return new Promise<Response>((resolve) => { finishRename = resolve })
    return response({})
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const threadsButton = () => document.querySelector<HTMLButtonElement>('button[aria-label="Switch Phoenix thread"]')!
    await act(async () => threadsButton().click())
    let menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const threadA = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('Thread A')); assert.ok(threadA)
    await act(async () => { threadA.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    const heading = Array.from(document.querySelectorAll('h2')).find((node) => node.textContent === 'Thread A'); assert.ok(heading)
    await act(async () => heading.dispatchEvent(new window.MouseEvent('click', { bubbles: true })))
    const titleInput = document.querySelector<HTMLInputElement>('input[aria-label="Conversation title"]'); assert.ok(titleInput)
    await act(async () => {
      titleInput.focus()
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(titleInput, 'Renamed A')
      titleInput.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'Renamed A' }) as unknown as Event)
      titleInput.blur()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.ok(finishRename, 'rename request is pending')

    await act(async () => threadsButton().click())
    menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const threadB = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('Thread B')); assert.ok(threadB)
    await act(async () => { threadB.click(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.match(document.body.textContent ?? '', /Thread B/)

    await act(async () => { finishRename?.(response({ error: 'rename failed' }, 500)); await new Promise((resolve) => setTimeout(resolve, 0)) })
    const currentTitle = Array.from(document.querySelectorAll('h2')).find((node) => node.textContent === 'Thread B')
    assert.ok(currentTitle, 'a late rename failure for Thread A must not overwrite Thread B title')
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix ignores delayed steering completion after starting a new thread', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  let finishSend: ((response: Response) => void) | undefined
  let finishSteer: ((response: Response) => void) | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true, capabilities: { session_lifecycle: [], turn_lifecycle: ['steer'], preserved_events: [], inputs: [], unsupported: [] } })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [] })
    if (body.action === 'phoenix.session.start') return response({ session_id: 'old', status: 'ready', messages: [] })
    if (body.action === 'phoenix.turn.send') return new Promise<Response>((resolve) => { finishSend = resolve })
    if (body.action === 'phoenix.turn.steer') return new Promise<Response>((resolve) => { finishSteer = resolve })
    if (body.action === 'phoenix.session.read') return response({ session_id: 'old', status: 'ready', messages: [{ role: 'user', text: 'old question' }, { role: 'user', text: 'OLD GUIDANCE' }], events: [] })
    return response({ session_id: 'old', status: 'ready', messages: [] })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]'); assert.ok(input)
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set?.call(input, 'old question')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'old question' }) as unknown as Event)
      input.form!.requestSubmit()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.ok(finishSend, 'turn POST is pending')
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set?.call(input, 'guidance')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'guidance' }) as unknown as Event)
      input.form!.requestSubmit()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.ok(finishSteer, 'steer POST is pending')
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Switch Phoenix thread"]')!.click())
    const menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const newButton = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('New')); assert.ok(newButton)
    await act(async () => newButton.click())
    assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0)
    await act(async () => {
      finishSteer?.(response({ session_id: 'old', status: 'steered' }))
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0, 'stale steer completion must not repopulate the new thread')
  } finally {
    finishSend?.(response({ session_id: 'old', status: 'ready', messages: [] }))
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix ignores a completed old turn after starting a new thread', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  let finishSend: ((response: Response) => void) | undefined
  const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'phoenix.status') return response({ enabled: true, available: true })
    if (body.action === 'phoenix.models.list') return response({ models: [] })
    if (body.action === 'phoenix.session.list') return response({ sessions: [] })
    if (body.action === 'phoenix.session.start') return response({ session_id: 'old', status: 'ready', messages: [] })
    if (body.action === 'phoenix.turn.send') return new Promise<Response>((resolve) => { finishSend = resolve })
    return response({ session_id: 'old', status: 'ready', messages: [] })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]'); assert.ok(input)
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set?.call(input, 'old question')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'old question' }) as unknown as Event)
    })
    await act(async () => { input.form!.requestSubmit(); await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.ok(finishSend, 'turn POST is pending')
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Switch Phoenix thread"]')!.click())
    const menu = document.querySelector('[aria-label="Phoenix threads"]'); assert.ok(menu)
    const newButton = Array.from(menu.querySelectorAll('button')).find((button) => button.textContent?.includes('New')); assert.ok(newButton)
    await act(async () => newButton.click())
    assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0)
    await act(async () => {
      finishSend?.(response({ session_id: 'old', status: 'ready', messages: [{ role: 'assistant', text: 'OLD THREAD RESPONSE' }] }))
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0)
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

for (const transition of ['dock', 'close', 'identity', 'unmount'] as const) {
  test(`Phoenix preserves request ownership across ${transition} transitions`, async () => {
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    const originalFetch = globalThis.fetch
    const actions: string[] = []
    let finishRequest: ((response: Response) => void) | undefined
    const response = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      actions.push(action)
      if (action === 'phoenix.status') return response({ enabled: true, available: true })
      if (action === 'phoenix.models.list') return response({ models: [] })
      if (action === 'phoenix.session.list') return response({ sessions: [] })
      if (action === (transition === 'unmount' ? 'phoenix.session.start' : 'phoenix.turn.send')) {
        return new Promise<Response>((resolve) => { finishRequest = resolve })
      }
      return response({ session_id: 'old', status: 'ready', messages: [] })
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    let unmounted = false
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
      const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]'); assert.ok(input)
      await act(async () => {
        Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set?.call(input, 'old question')
        input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'old question' }) as unknown as Event)
      })
      await act(async () => { input.form!.requestSubmit(); await new Promise((resolve) => setTimeout(resolve, 0)) })
      assert.ok(finishRequest, 'request must be pending before the transition')
      if (transition === 'dock') {
        await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Dock Phoenix right"]')!.click())
      } else if (transition === 'close') {
        await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Close Phoenix panel"]')!.click())
        assert.equal(document.querySelector('[aria-label="Phoenix session"]'), null)
      } else if (transition === 'identity') {
        __setBrowserSessionStateForTests({ status: 'unauthenticated' })
        await view.rerender(<PhoenixAvailability />)
        __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'new-user' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-new' })
        await view.rerender(<PhoenixAvailability />)
      } else {
        await view.unmount()
        unmounted = true
      }
      await act(async () => {
        finishRequest?.(response({ session_id: 'old', status: 'ready', messages: [{ role: 'assistant', text: 'OLD THREAD RESPONSE' }] }))
        await new Promise((resolve) => setTimeout(resolve, 0))
      })
      if (transition === 'unmount') {
        assert.equal(actions.includes('phoenix.turn.send'), false, 'unmounted session-start completion must not dispatch a turn')
      } else {
        if (transition === 'close') {
          await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
        }
        const currentInput = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]'); assert.ok(currentInput)
        assert.equal(currentInput.disabled, false, 'completion must release the pending send')
        assert.equal(currentInput.value, '', 'old drafts must not be restored after identity reset')
        const panel = document.querySelector('[aria-label="Phoenix session"]'); assert.ok(panel)
        if (transition === 'identity') {
          assert.equal(panel.querySelectorAll('[data-phoenix-message]').length, 0)
          assert.equal(panel.querySelector('[role="alert"]'), null)
        } else {
          assert.match(panel.textContent ?? '', /OLD THREAD RESPONSE/)
        }
      }
    } finally {
      if (!unmounted) await view.unmount()
      globalThis.fetch = originalFetch
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}
