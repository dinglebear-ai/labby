import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom } from '../../lib/testing/dom-install.ts'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
installTestDom()
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })

test('Phoenix surfaces model discovery failures after available status', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'phoenix.status') return new Response(JSON.stringify({ available: true }), { status: 200 })
    if (action === 'phoenix.models.list') return new Response(JSON.stringify({ message: 'Model catalog unavailable' }), { status: 503 })
    return new Response(JSON.stringify({ sessions: [] }), { status: 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    assert.match(document.querySelector('[aria-label="Phoenix session"]')?.textContent ?? '', /Model catalog unavailable/)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix cancels pending model discovery quietly on unmount', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  let modelSignal: AbortSignal | null | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'phoenix.status') return new Response(JSON.stringify({ available: true }), { status: 200 })
    if (action === 'phoenix.models.list') {
      modelSignal = init?.signal
      return new Promise<Response>((_resolve, reject) => modelSignal?.addEventListener('abort', () => reject(new DOMException('cancelled', 'AbortError')), { once: true }))
    }
    return new Response(JSON.stringify({ sessions: [] }), { status: 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    assert.ok(modelSignal, 'model discovery started')
    await view.unmount()
    assert.equal(modelSignal.aborted, true)
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
  } finally {
    if (view.container.isConnected) await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
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
    assert.ok(panel.querySelector('button[aria-label="Copy preceding prompt to new conversation"]'))
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

for (const fails of [false, true]) {
  test(`Phoenix ignores stale steering ${fails ? 'failures' : 'successes'} after a new conversation`, async () => {
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    const originalFetch = globalThis.fetch
    const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
    let finishSend!: (response: Response) => void
    let finishSteer!: (response: Response) => void
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      if (action === 'phoenix.status') return response({ available: true, capabilities: { turn_lifecycle: ['steer'], unsupported: [] } })
      if (action === 'phoenix.models.list') return response({ models: [] })
      if (action === 'phoenix.session.list') return response({ sessions: [] })
      if (action === 'phoenix.turn.send') return new Promise<Response>(resolve => { finishSend = resolve })
      if (action === 'phoenix.turn.steer') return new Promise<Response>(resolve => { finishSteer = resolve })
      return response({ session_id: 'old', status: 'ready', messages: [{ role: 'assistant', text: 'OLD STEERING' }] })
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    const enter = async (value: string) => act(async () => {
      const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]')!
      Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')!.set!.call(input, value)
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
    })
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      await enter('original question')
      await act(async () => { document.querySelector<HTMLTextAreaElement>('textarea')!.form!.requestSubmit(); await new Promise(resolve => setTimeout(resolve, 0)) })
      assert.ok(finishSend)
      await enter('old guidance')
      await act(async () => { document.querySelector<HTMLTextAreaElement>('textarea')!.form!.requestSubmit(); await new Promise(resolve => setTimeout(resolve, 0)) })
      assert.ok(finishSteer)
      await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
      const newButton = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(button => button.textContent?.includes('New'))!
      await act(async () => newButton.click())
      await act(async () => {
        finishSteer(response(fails ? { message: 'OLD ERROR' } : { status: 'steered' }, fails ? 500 : 200))
        finishSend(response({ session_id: 'old', messages: [{ role: 'assistant', text: 'OLD TURN' }] }))
        await new Promise(resolve => setTimeout(resolve, 0))
      })
      assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0)
      assert.equal(document.querySelector<HTMLTextAreaElement>('textarea')!.value, '')
      assert.doesNotMatch(document.body.textContent ?? '', /OLD STEERING|OLD ERROR|Guidance added/)
    } finally {
      globalThis.fetch = originalFetch
      await view.unmount()
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}

test('copying a prompt to a fresh conversation is explicit and waits for review before sending', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    actions.push(action)
    const body = action === 'phoenix.status' ? { available: true }
      : action === 'phoenix.models.list' ? { models: [] }
      : action === 'phoenix.session.list' ? { sessions: [{ session_id: 'existing', title: 'Existing' }] }
      : { session_id: 'existing', messages: [{ role: 'user', text: 'Earlier context' }, { role: 'assistant', text: 'Earlier answer' }, { role: 'user', text: 'Follow-up' }, { role: 'assistant', text: 'Latest answer' }] }
    return new Response(JSON.stringify(body), { status: 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
    const existing = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(button => button.textContent?.includes('Existing'))!
    assert.ok(existing)
    await act(async () => { existing.click(); await new Promise(resolve => setTimeout(resolve, 0)) })
    const copy = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Copy prompt to new conversation"]')].at(-1)!
    assert.ok(copy)
    await act(async () => copy.click())
    assert.equal(document.querySelector<HTMLTextAreaElement>('textarea')!.value, 'Follow-up')
    assert.equal(document.querySelectorAll('[data-phoenix-message]').length, 0)
    assert.match(document.body.textContent ?? '', /Earlier messages and attachments are not included/)
    assert.equal(actions.includes('phoenix.turn.send'), false)
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

for (const fails of [false, true]) {
  test(`Phoenix fences stale interruption ${fails ? 'failures' : 'successes'} and preserves a newer interruption`, async () => {
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    const originalFetch = globalThis.fetch
    const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
    const turns: Array<(response: Response) => void> = []
    const interrupts: Array<(response: Response) => void> = []
    let started = 0
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      if (action === 'phoenix.status') return response({ available: true, capabilities: { turn_lifecycle: ['interrupt'], unsupported: [] } })
      if (action === 'phoenix.models.list') return response({ models: [] })
      if (action === 'phoenix.session.list') return response({ sessions: [] })
      if (action === 'phoenix.session.start') return response({ session_id: `session-${++started}`, messages: [] })
      if (action === 'phoenix.turn.send') return new Promise<Response>(resolve => { turns.push(resolve) })
      if (action === 'phoenix.turn.interrupt') return new Promise<Response>(resolve => { interrupts.push(resolve) })
      return response({ messages: [] })
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    const send = async (value: string) => {
      await act(async () => {
        const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]')!
        Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')!.set!.call(input, value)
        input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
      })
      await act(async () => {
        document.querySelector<HTMLTextAreaElement>('textarea')!.form!.requestSubmit()
        await new Promise(resolve => setTimeout(resolve, 0))
      })
    }
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      await send('old question')
      await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Stop Phoenix"]')!.click())
      assert.equal(interrupts.length, 1)
      await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
      const newButton = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(button => button.textContent?.includes('New'))!
      await act(async () => newButton.click())
      await send('new question')
      const stop = document.querySelector<HTMLButtonElement>('[aria-label="Stop Phoenix"]')!
      assert.ok(stop)
      assert.equal(stop.disabled, false)
      await act(async () => stop.click())
      assert.equal(interrupts.length, 2)
      await act(async () => {
        interrupts[0](response(fails ? { message: 'OLD INTERRUPT ERROR' } : { status: 'interrupting' }, fails ? 500 : 200))
        await new Promise(resolve => setTimeout(resolve, 0))
      })
      assert.doesNotMatch(document.body.textContent ?? '', /OLD INTERRUPT ERROR|Stopping the active turn/)
      assert.equal(document.querySelector<HTMLButtonElement>('[aria-label="Stopping Phoenix"]')?.disabled, true)
      await act(async () => {
        interrupts[1](response({ status: 'interrupting' }))
        turns.forEach(resolve => resolve(response({ messages: [] })))
        await new Promise(resolve => setTimeout(resolve, 0))
      })
      assert.match(document.body.textContent ?? '', /Stopping the active turn/)
    } finally {
      globalThis.fetch = originalFetch
      await view.unmount()
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}

for (const control of ['steer', 'interrupt'] as const) {
  for (const fails of [false, true]) {
    test(`Phoenix releases obsolete ${control} on a same-session send and fences late ${fails ? 'failures' : 'successes'}`, async () => {
      __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
      const originalFetch = globalThis.fetch
      const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
      const turns: Array<(response: Response) => void> = []
      const controls: Array<(response: Response) => void> = []
      let started = 0
      globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
        const { action } = JSON.parse(String(init?.body)) as { action: string }
        if (action === 'phoenix.status') return response({ available: true, capabilities: { turn_lifecycle: [control], unsupported: [] } })
        if (action === 'phoenix.models.list') return response({ models: [] })
        if (action === 'phoenix.session.list') return response({ sessions: [] })
        if (action === 'phoenix.session.start') return response({ session_id: `session-${++started}`, messages: [] })
        if (action === 'phoenix.turn.send') return new Promise<Response>(resolve => { turns.push(resolve) })
        if (action === `phoenix.turn.${control}`) return new Promise<Response>(resolve => { controls.push(resolve) })
        return response({ messages: [] })
      }) as typeof fetch
      const { PhoenixAvailability } = await import('./console-global-tools.tsx')
      const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
      const view = await renderClient(<PhoenixAvailability />)
      const send = async (value: string) => {
        await act(async () => {
          const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]')!
          Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')!.set!.call(input, value)
          input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
        })
        await act(async () => {
          document.querySelector<HTMLTextAreaElement>('textarea')!.form!.requestSubmit()
          await new Promise(resolve => setTimeout(resolve, 0))
        })
      }
      const controlTurn = async () => {
        if (control === 'steer') await send('guidance')
        else {
          const stop = document.querySelector<HTMLButtonElement>('[aria-label="Stop Phoenix"]')
          assert.ok(stop, 'the current turn exposes an available interruption')
          await act(async () => stop.click())
        }
      }
      try {
        await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
        await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
        await send('old question')
        await controlTurn()
        assert.equal(controls.length, 1)
        await act(async () => {
          turns[0](response({ session_id: 'session-1', messages: [] }))
          await new Promise(resolve => setTimeout(resolve, 0))
        })
        await send('next question')
        assert.equal(started, 1, 'the next send keeps the same session')
        await controlTurn()
        assert.equal(controls.length, 2, 'a new turn releases the obsolete pending control')
        await act(async () => {
          controls[0](response(fails ? { message: 'OLD CONTROL ERROR' } : { status: control === 'steer' ? 'steered' : 'interrupting' }, fails ? 500 : 200))
          await new Promise(resolve => setTimeout(resolve, 0))
        })
        assert.doesNotMatch(document.body.textContent ?? '', /OLD CONTROL ERROR|Stopping the active turn|Guidance added/)
        if (control === 'interrupt') assert.equal(document.querySelector<HTMLButtonElement>('[aria-label="Stopping Phoenix"]')?.disabled, true)
        else {
          await send('guidance while pending')
          assert.equal(controls.length, 2, 'the older completion cannot release the newer steer')
        }
        await act(async () => {
          controls[1](response({ status: control === 'steer' ? 'steered' : 'interrupting' }))
          turns[1](response({ messages: [] }))
          await new Promise(resolve => setTimeout(resolve, 0))
        })
        assert.match(document.body.textContent ?? '', control === 'interrupt' ? /Stopping the active turn/ : /Guidance added/)
      } finally {
        globalThis.fetch = originalFetch
        await view.unmount()
        __setBrowserSessionStateForTests({ status: 'unauthenticated' })
      }
    })
  }
}

for (const transition of ['logout', 'account status failure', 'workspace list failure'] as const) {
  test(`Phoenix clears private conversation metadata after ${transition}`, async () => {
    const authenticated = (sub: string, projectId: string) => ({ status: 'authenticated' as const, user: { sub }, projectId, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    __setBrowserSessionStateForTests(authenticated('operator-A', 'project-A'))
    const originalFetch = globalThis.fetch
    let replaced = false
    const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      if (action === 'phoenix.status') return replaced && transition === 'account status failure' ? response({ message: 'Status unavailable for B' }, 503) : response({ available: true })
      if (action === 'phoenix.models.list') return response({ models: [] })
      if (action === 'phoenix.session.list') return replaced ? response({ message: 'History unavailable for B' }, 503) : response({ sessions: [{ session_id: 'private-A', title: 'Private conversation A' }] })
      return response({ session_id: 'private-A', messages: [] })
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
      const privateThread = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(button => button.textContent === 'Private conversation A')!
      assert.ok(privateThread, 'authority A history loaded')
      await act(async () => privateThread.click())
      assert.equal(document.querySelector('[title="Click to rename"]')?.textContent, 'Private conversation A')
      await act(async () => document.querySelector<HTMLElement>('[title="Click to rename"]')!.click())
      assert.equal(document.querySelector<HTMLInputElement>('[aria-label="Conversation title"]')!.value, 'Private conversation A')
      replaced = true
      __setBrowserSessionStateForTests(transition === 'logout' ? { status: 'unauthenticated' } : authenticated(transition === 'account status failure' ? 'operator-B' : 'operator-A', 'project-B'))
      await view.rerender(<PhoenixAvailability />)
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      assert.ok(document.querySelector('[aria-label="Conversation title"]') === null, 'old title editor is revoked')
      assert.equal(document.querySelector('[title="Click to rename"]')?.textContent, 'Phoenix')
      if (document.querySelector('[aria-label="Phoenix threads"]') === null) await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
      assert.doesNotMatch(document.body.textContent ?? '', /Private conversation A/)
      assert.match(document.querySelector('[aria-label="Phoenix threads"]')?.textContent ?? '', /No previous Phoenix threads/)
    } finally {
      await view.unmount()
      globalThis.fetch = originalFetch
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}

test('Phoenix rejects a deferred history publication from the prior browser identity', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-A' }, projectId: 'project-A', expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const { phoenixApi } = await import('../../lib/api/phoenix-client.ts')
  const originalList = phoenixApi.list
  let finishOld!: (result: Awaited<ReturnType<typeof phoenixApi.list>>) => void
  let oldSignal: AbortSignal | undefined
  let calls = 0
  const summary = (title: string) => ({ session_id: title, title, preview: title, message_count: 1, turn_status: 'ready' as const })
  phoenixApi.list = (signal) => ++calls === 1 ? new Promise(resolve => { oldSignal = signal; finishOld = resolve }) : Promise.resolve({ sessions: [summary('Conversation B')] })
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    return new Response(JSON.stringify(action === 'phoenix.status' ? { available: true } : { models: [] }))
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    assert.ok(finishOld)
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator-B' }, projectId: 'project-B', expiresAt: Date.now() + 60_000, csrfToken: 'other-csrf' })
    await view.rerender(<PhoenixAvailability />)
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
    assert.equal(oldSignal?.aborted, true, 'prior history request is revoked')
    assert.match(document.querySelector('[aria-label="Phoenix threads"]')?.textContent ?? '', /Conversation B/)
    // The transport may already have completed when cancellation arrives.
    await act(async () => finishOld({ sessions: [summary('Private conversation A')] }))
    assert.match(document.querySelector('[aria-label="Phoenix threads"]')?.textContent ?? '', /Conversation B/)
    assert.doesNotMatch(document.body.textContent ?? '', /Private conversation A/)
  } finally {
    await view.unmount()
    phoenixApi.list = originalList
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('Phoenix retains current history across transport-only session refresh', async () => {
  const authenticated = (csrfToken: string) => ({ status: 'authenticated' as const, user: { sub: 'operator-A' }, projectId: 'project-A', expiresAt: Date.now() + 60_000, csrfToken })
  __setBrowserSessionStateForTests(authenticated('csrf'))
  const originalFetch = globalThis.fetch
  let lists = 0
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'phoenix.session.list') lists += 1
    return new Response(JSON.stringify(action === 'phoenix.status' ? { available: true } : action === 'phoenix.models.list' ? { models: [] } : { sessions: [{ session_id: 'A', title: 'Current conversation A' }] }))
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
    assert.match(document.querySelector('[aria-label="Phoenix threads"]')?.textContent ?? '', /Current conversation A/)
    __setBrowserSessionStateForTests(authenticated('rotated-csrf'))
    await view.rerender(<PhoenixAvailability />)
    assert.match(document.querySelector('[aria-label="Phoenix threads"]')?.textContent ?? '', /Current conversation A/)
    assert.equal(lists, 1, 'transport refresh preserves the current cache and request owner')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

for (const transition of ['thread', 'identity', 'read failure'] as const) {
  test(`Phoenix discards a deferred attachment after ${transition} changes`, async () => {
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    const originalFetch = globalThis.fetch
    const originalReader = globalThis.FileReader
    let finishRead!: () => void
    class DeferredReader extends window.FileReader {
      override readAsDataURL() {
        finishRead = () => {
          if (transition === 'read failure') {
            this.onerror?.(new window.ProgressEvent('error') as unknown as ProgressEvent<FileReader>)
            return
          }
          Object.defineProperty(this, 'result', { configurable: true, value: 'data:text/plain;base64,cHJpdmF0ZQ==' })
          this.onload?.(new window.ProgressEvent('load') as unknown as ProgressEvent<FileReader>)
        }
      }
    }
    globalThis.FileReader = DeferredReader
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      return new Response(JSON.stringify(action === 'phoenix.status' ? { available: true } : action === 'phoenix.models.list' ? { models: [] } : { sessions: [] }))
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      const picker = document.querySelector<HTMLInputElement>('[aria-label="Attach image or file"]')!
      Object.defineProperty(picker, 'files', { configurable: true, value: [new window.File(['private'], 'private.txt', { type: 'text/plain' })] })
      await act(async () => picker.dispatchEvent(new window.Event('change', { bubbles: true })))
      assert.ok(finishRead)
      if (transition === 'thread') {
        await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
        const newButton = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(button => button.textContent?.includes('New'))!
        await act(async () => newButton.click())
      } else if (transition === 'identity') {
        __setBrowserSessionStateForTests({ status: 'unauthenticated' })
        await view.rerender(<PhoenixAvailability />)
        __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'other-operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'other-csrf' })
        await view.rerender(<PhoenixAvailability />)
      }
      await act(async () => { finishRead(); await new Promise(resolve => setTimeout(resolve, 0)) })
      assert.ok(document.querySelector('[aria-label="Phoenix attachments"]') === null)
      if (transition === 'read failure') assert.match(document.body.textContent ?? '', /could not read the selected attachments/)
    } finally {
      await view.unmount()
      globalThis.fetch = originalFetch
      globalThis.FileReader = originalReader
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}

test('Phoenix ignores a rename failure after opening another conversation', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  let failRename!: (response: Response) => void
  const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'phoenix.status') return response({ available: true })
    if (action === 'phoenix.models.list') return response({ models: [] })
    if (action === 'phoenix.session.list') return response({ sessions: [{ session_id: 'A', title: 'Conversation A' }, { session_id: 'B', title: 'Conversation B' }] })
    if (action === 'phoenix.session.rename') return new Promise<Response>(resolve => { failRename = resolve })
    return response({ messages: [] })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  const openThread = async (title: string) => {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
    const button = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(item => item.textContent === title)!
    assert.ok(button)
    await act(async () => { button.click(); await new Promise(resolve => setTimeout(resolve, 0)) })
  }
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    await openThread('Conversation A')
    await act(async () => document.querySelector<HTMLElement>('[title="Click to rename"]')!.click())
    const titleInput = document.querySelector<HTMLInputElement>('[aria-label="Conversation title"]')!
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(titleInput, 'Renamed A')
      titleInput.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'Renamed A' }) as unknown as Event)
    })
    await act(async () => titleInput.blur())
    assert.ok(failRename)
    await openThread('Conversation B')
    await act(async () => { failRename(response({ message: 'OLD RENAME ERROR' }, 500)); await new Promise(resolve => setTimeout(resolve, 0)) })
    assert.equal(document.querySelector('[title="Click to rename"]')?.textContent, 'Conversation B')
    assert.doesNotMatch(document.body.textContent ?? '', /OLD RENAME ERROR/)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

for (const transition of ['identity replacement', 'A-B-A transition', 'batched A-B-A admission', 'batched A-B-A completion'] as const) {
  for (const fails of [false, true]) {
    test(`Phoenix preserves newer diagnostics after an obsolete ${fails ? 'failure' : 'success'} and ${transition}`, async () => {
      const authenticated = (sub: string) => ({ status: 'authenticated' as const, user: { sub }, projectId: `project-${sub}`, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
      __setBrowserSessionStateForTests(authenticated('A'))
      const originalFetch = globalThis.fetch
      const { phoenixApi } = await import('../../lib/api/phoenix-client.ts')
      const originalDiagnostics = phoenixApi.diagnostics
      type Diagnostics = Awaited<ReturnType<typeof phoenixApi.diagnostics>>
      const pending: Array<{ resolve: (result: Diagnostics) => void; reject: (reason: Error) => void }> = []
      phoenixApi.diagnostics = () => new Promise((resolve, reject) => { pending.push({ resolve, reject }) })
      globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
        const { action } = JSON.parse(String(init?.body)) as { action: string }
        const body = action === 'phoenix.status' ? { available: true, capabilities: { diagnostics: ['config'] } }
          : action === 'phoenix.models.list' ? { models: [] } : { sessions: [] }
        return new Response(JSON.stringify(body))
      }) as typeof fetch
      const { PhoenixAvailability } = await import('./console-global-tools.tsx')
      const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
      const view = await renderClient(<PhoenixAvailability />)
      const readButton = () => document.querySelector<HTMLButtonElement>('[aria-label="Read Phoenix diagnostics"]')!
      try {
        await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
        await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
        await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Phoenix settings"]')!.click())
        await act(async () => readButton().click())
        assert.equal(pending.length, 1)
        const finishOld = () => {
          if (fails) pending[0].reject(new Error('OLD DIAGNOSTICS ERROR'))
          else pending[0].resolve({ account: 'private diagnostics from A' })
        }
        if (transition.startsWith('batched')) {
          await act(async () => {
            __setBrowserSessionStateForTests(authenticated('B'))
            __setBrowserSessionStateForTests(authenticated('A'))
            if (transition === 'batched A-B-A completion') finishOld()
            await view.rerender(<PhoenixAvailability />)
            await new Promise(resolve => setTimeout(resolve, 0))
          })
          assert.doesNotMatch(document.body.textContent ?? '', /OLD DIAGNOSTICS ERROR|Diagnostics ready: account/)
          assert.equal(readButton().disabled, false, 'a batched authority round-trip revokes obsolete diagnostics admission')
        } else {
          for (const sub of transition === 'A-B-A transition' ? ['B', 'A'] : ['B']) {
            __setBrowserSessionStateForTests(authenticated(sub))
            await view.rerender(<PhoenixAvailability />)
            await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
          }
        }
        await act(async () => readButton().click())
        assert.equal(pending.length, 2)
        await act(async () => {
          if (transition !== 'batched A-B-A completion') finishOld()
          await new Promise(resolve => setTimeout(resolve, 0))
        })
        assert.doesNotMatch(document.body.textContent ?? '', /OLD DIAGNOSTICS ERROR|Diagnostics ready: account/)
        assert.equal(readButton().disabled, true, 'the older completion cannot release the current diagnostics request')
        await act(async () => pending[1].resolve({ config: {} }))
        assert.match(document.body.textContent ?? '', /Diagnostics ready: config/)
        assert.equal(readButton().disabled, false)
      } finally {
        await view.unmount()
        phoenixApi.diagnostics = originalDiagnostics
        globalThis.fetch = originalFetch
        __setBrowserSessionStateForTests({ status: 'unauthenticated' })
      }
    })
  }
}

test('Phoenix reports current diagnostics failures and releases the read control', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    const body = action === 'phoenix.status' ? { available: true, capabilities: { diagnostics: ['config'] } }
      : action === 'phoenix.models.list' ? { models: [] }
      : action === 'phoenix.diagnostics.read' ? { message: 'Diagnostics temporarily unavailable' } : { sessions: [] }
    return new Response(JSON.stringify(body), { status: action === 'phoenix.diagnostics.read' ? 503 : 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Phoenix settings"]')!.click())
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Read Phoenix diagnostics"]')!.click()
      await new Promise(resolve => setTimeout(resolve, 0))
    })
    assert.match(document.body.textContent ?? '', /Diagnostics temporarily unavailable/)
    assert.equal(document.querySelector<HTMLButtonElement>('[aria-label="Read Phoenix diagnostics"]')!.disabled, false)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

for (const fails of [false, true]) {
  test(`Phoenix handles current thread-close ${fails ? 'failure visibly without deleting the thread' : 'success'}`, async () => {
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
      const { action } = JSON.parse(String(init?.body)) as { action: string }
      const body = action === 'phoenix.status' ? { available: true }
        : action === 'phoenix.models.list' ? { models: [] }
        : action === 'phoenix.session.close' ? fails ? { message: 'Thread could not be closed' } : { session_id: 'A', status: 'closed' }
        : { sessions: [{ session_id: 'A', title: 'Conversation A' }] }
      return new Response(JSON.stringify(body), { status: fails && action === 'phoenix.session.close' ? 503 : 200 })
    }) as typeof fetch
    const { PhoenixAvailability } = await import('./console-global-tools.tsx')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const view = await renderClient(<PhoenixAvailability />)
    try {
      await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
      await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
      await act(async () => {
        document.querySelector<HTMLButtonElement>('[aria-label="Close Conversation A"]')!.click()
        await new Promise(resolve => setTimeout(resolve, 0))
      })
      assert.equal(document.querySelector('[aria-label="Close Conversation A"]') !== null, fails)
      if (fails) assert.match(document.body.textContent ?? '', /Thread could not be closed/)
    } finally {
      await view.unmount()
      globalThis.fetch = originalFetch
      __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    }
  })
}

for (const transition of ['thread', 'identity', 'batched identity'] as const) {
  for (const fails of [false, true]) {
    test(`Phoenix ignores late thread-close ${fails ? 'failures' : 'successes'} after a ${transition} change`, async () => {
      const authenticated = (sub: string) => ({ status: 'authenticated' as const, user: { sub }, projectId: `project-${sub}`, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
      __setBrowserSessionStateForTests(authenticated('A'))
      const originalFetch = globalThis.fetch
      const { phoenixApi } = await import('../../lib/api/phoenix-client.ts')
      const originalClose = phoenixApi.close
      let finishClose!: () => void
      phoenixApi.close = () => new Promise((resolve, reject) => {
        finishClose = () => fails ? reject(new Error('OLD CLOSE ERROR')) : resolve({ session_id: 'A', status: 'closed' })
      })
      globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
        const { action, params } = JSON.parse(String(init?.body)) as { action: string; params?: { session_id?: string } }
        const body = action === 'phoenix.status' ? { available: true }
          : action === 'phoenix.models.list' ? { models: [] }
          : action === 'phoenix.session.list' ? { sessions: [{ session_id: 'A', title: 'Conversation A' }, { session_id: 'B', title: 'Conversation B' }] }
          : { session_id: params?.session_id, messages: [{ role: 'assistant', text: `Conversation ${params?.session_id} reply` }] }
        return new Response(JSON.stringify(body))
      }) as typeof fetch
      const { PhoenixAvailability } = await import('./console-global-tools.tsx')
      const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
      const view = await renderClient(<PhoenixAvailability />)
      const openThread = async (title: string) => {
        if (document.querySelector('[aria-label="Phoenix threads"]') === null) await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
        const button = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Phoenix threads"] button')].find(item => item.textContent === title)!
        assert.ok(button)
        await act(async () => { button.click(); await new Promise(resolve => setTimeout(resolve, 0)) })
      }
      try {
        await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Ask Phoenix"]')!.click())
        await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
        await openThread('Conversation A')
        await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Switch Phoenix thread"]')!.click())
        await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Close Conversation A"]')!.click())
        assert.ok(finishClose)
        if (transition === 'batched identity') {
          await act(async () => {
            __setBrowserSessionStateForTests(authenticated('B'))
            __setBrowserSessionStateForTests(authenticated('A'))
            finishClose()
            await view.rerender(<PhoenixAvailability />)
            await new Promise(resolve => setTimeout(resolve, 0))
          })
          assert.match(document.body.textContent ?? '', /Conversation A reply/)
        } else {
          if (transition === 'identity') {
            __setBrowserSessionStateForTests(authenticated('B'))
            await view.rerender(<PhoenixAvailability />)
            await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
          }
          await openThread('Conversation B')
          assert.match(document.body.textContent ?? '', /Conversation B reply/)
          await act(async () => { finishClose(); await new Promise(resolve => setTimeout(resolve, 0)) })
          assert.match(document.body.textContent ?? '', /Conversation B reply/)
        }
        assert.doesNotMatch(document.body.textContent ?? '', /OLD CLOSE ERROR/)
      } finally {
        await view.unmount()
        phoenixApi.close = originalClose
        globalThis.fetch = originalFetch
        __setBrowserSessionStateForTests({ status: 'unauthenticated' })
      }
    })
  }
}
