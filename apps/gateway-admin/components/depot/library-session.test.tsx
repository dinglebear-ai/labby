import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'

const dom = installTestDom()
Object.defineProperty(globalThis, 'NodeFilter', { value: dom.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: dom.HTMLInputElement, configurable: true })
let LibraryPageContent: typeof import('./library-page-content.tsx').LibraryPageContent
test.before(async () => { ({ LibraryPageContent } = await import('./library-page-content.tsx')) })
const envelope = (result: unknown) => Response.json(result)
function deferred() {
  let resolve!: (response: Response) => void
  let reject!: (error: Error) => void
  const promise = new Promise<Response>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const flush = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })
const page = (id: string) => <SearchParamsContext.Provider value={new URLSearchParams({ artifact: id })}><LibraryPageContent /></SearchParamsContext.Provider>

test('detail responses and retained details cannot cross selection or session boundaries', async () => {
  const originalFetch = globalThis.fetch
  const reads: ReturnType<typeof deferred>[] = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    if (body.action === 'artifacts.get_remote') { const read = deferred(); reads.push(read); return read.promise }
    return envelope({ artifacts: [] })
  }
  const view = await renderClient(page('alpha'))
  try {
    await flush()
    await view.rerender(page('bravo'))
    await act(async () => reads[1].resolve(envelope({ artifact: { id: 'bravo', title: 'Bravo private details' } })))
    assert.match(document.body.textContent ?? '', /Bravo private details/)
    await view.rerender(page('charlie'))
    assert.doesNotMatch(document.body.textContent ?? '', /Bravo private details/)
    assert.equal(document.body.querySelector('a[href="/depot?artifact=bravo"]'), null)
    await act(async () => reads[2].resolve(envelope({ artifact: { id: 'charlie', title: 'Charlie private details' } })))
    await act(async () => reads[0].resolve(envelope({ artifact: { id: 'alpha', title: 'Alpha stale details' } })))
    assert.doesNotMatch(document.body.textContent ?? '', /Alpha stale details/)
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await view.rerender(page('charlie'))
    assert.doesNotMatch(document.body.textContent ?? '', /Charlie private details/)
    await flush()
    assert.equal(reads.length, 4)
    await act(async () => reads[3].reject(new Error('new session denied')))
    assert.doesNotMatch(document.body.textContent ?? '', /Bravo private details|Charlie private details|Alpha stale details/)
    assert.match(document.body.textContent ?? '', /Artifact details are unavailable/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('late page failures and successes cannot overwrite a new query', async () => {
  const originalFetch = globalThis.fetch
  const pending: ReturnType<typeof deferred>[] = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    if (body.params.cursor) { const read = deferred(); pending.push(read); return read.promise }
    return envelope({ artifacts: [{ id: body.params.query || 'initial', title: body.params.query || 'Initial item' }], nextCursor: 'next' })
  }
  const view = await renderClient(page(''))
  try {
    await flush()
    const more = () => [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Load 50 more'))!
    act(() => more().click())
    await flush()
    const input = view.container.querySelector('input')!
    const key = Object.keys(input).find(key => key.startsWith('__reactProps$'))!
    const props = (input as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[key]
    await act(async () => props.onChange({ target: { value: 'New query' } }))
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 350)) })
    await act(async () => pending[0].resolve(envelope({ artifacts: [{ id: 'stale', title: 'Stale page' }] })))
    assert.doesNotMatch(view.container.textContent ?? '', /Stale page/)
    assert.match(view.container.textContent ?? '', /New query/)
    act(() => more().click())
    await flush()
    await act(async () => props.onChange({ target: { value: 'Final query' } }))
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 350)) })
    await act(async () => pending[1].reject(new Error('obsolete page failure')))
    assert.equal(view.container.querySelector('[role="alert"]'), null)
    assert.match(view.container.textContent ?? '', /Final query/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})
