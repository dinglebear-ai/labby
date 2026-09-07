import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { readFile } from 'node:fs/promises'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store.ts'
import type { AuthoritySnapshot } from '@/lib/auth/authority.ts'
import { StashPageContent } from './stash-page-content.tsx'

installTestDom()
const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})
const teamAuthority: AuthoritySnapshot = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team', id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [{ id: 'project-1', role: 'manager' }], capabilities: ['scope.read'], generation: 1 }
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
Object.defineProperty(globalThis, 'InputEvent', { configurable: true, value: window.InputEvent })
const file = (id: string) => ({ file_id: id, uri: `stash://me/files/${id}`, display_name: `${id}.txt`, size_bytes: 1, created_at: 1, updated_at: 1, owned: true })

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

test('Stash renders live data and appends the next cursor page', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 2, owned_shared_file_count: 0, owned_committed_bytes: 2, owned_reserved_bytes: 0 })
    return Response.json(url.searchParams.has('cursor') ? { files: [file('second')], next_cursor: null } : { files: [file('first')], next_cursor: 'next' })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /first\.txt/))
  const loadMore = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Load more'))
  assert.ok(loadMore)
  await act(async () => { loadMore.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => assert.match(view.container.textContent || '', /first\.txt.*second\.txt/))
  await view.unmount()
})

test('Stash can continue an empty page to a later result', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 2, owned_shared_file_count: 0, owned_committed_bytes: 2, owned_reserved_bytes: 0 })
    return Response.json(url.searchParams.has('cursor')
      ? { files: [file('needle')], next_cursor: null }
      : { files: [], next_cursor: 'next' })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const loadMore = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Load more'))
  assert.ok(loadMore)
  await act(async () => { loadMore.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => assert.match(view.container.textContent || '', /needle\.txt/))
  await view.unmount()
})

test('Stash accessibility gate covers names, status, upload equivalence, and reduced motion', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => String(input).includes('/stats')
    ? Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    : Response.json({ files: [], next_cursor: null })
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))

  const status = view.container.querySelector('[role="status"][aria-live="polite"]')
  assert.ok(status, 'mutation results require a polite live region')
  const uploadButtons = [...view.container.querySelectorAll('button')].filter(button => /upload|drop files here/i.test(button.textContent || ''))
  assert.equal(uploadButtons.length >= 2, true, 'button and drop-zone upload paths must both be operable controls')
  const dropTarget = uploadButtons.find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  dropTarget.focus()
  assert.equal(document.activeElement, dropTarget, 'drop-zone browse equivalent must receive keyboard focus')
  assert.ok(view.container.querySelector('input[type="file"]'), 'keyboard browse and drag/drop must share one file input')
  assert.ok(view.container.querySelector('input[placeholder="Search files…"]'), 'file search requires an accessible wrapping label')
  assert.ok(view.container.querySelector('[role="group"][aria-label="File layout"]'))

  const css = await readFile(new URL('../../app/globals.css', import.meta.url), 'utf8')
  assert.match(css, /@media \(prefers-reduced-motion: reduce\)/)
  assert.match(css, /animation:\s*none !important/)
  assert.match(css, /transition-duration:\s*1ms !important/)
  await view.unmount()
})

test('Stash preserves mixed upload outcomes and retries only the failed file', async () => {
  document.body.replaceChildren()
  let failedOnce = false
  const uploaded: string[] = []
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (url.pathname.endsWith('/uploads')) {
      const name = decodeURIComponent(new Headers(init?.headers).get('x-labby-stash-filename') || '')
      uploaded.push(name)
      if (name === 'retry.txt' && !failedOnce) {
        failedOnce = true
        return Response.json({ kind: 'conflict', message: 'try again' }, { status: 409 })
      }
      return Response.json({ file_id: name, uri: `stash://me/files/${name}` }, { status: 201 })
    }
    return Response.json({ files: [], next_cursor: null })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  const event = new window.Event('drop', { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: { files: [new File(['ok'], 'ok.txt'), new File(['retry'], 'retry.txt')] } })
  await act(async () => { dropTarget.dispatchEvent(event) })
  await waitFor(() => { assert.match(view.container.textContent || '', /ok\.txt — complete/); assert.match(view.container.textContent || '', /retry\.txt — failed/) })
  const retry = [...view.container.querySelectorAll('button')].find(button => /retry upload/i.test(button.textContent || ''))
  assert.ok(retry)
  await act(async () => { retry.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => { assert.deepEqual(uploaded, ['ok.txt', 'retry.txt', 'retry.txt']); assert.match(view.container.textContent || '', /retry\.txt — complete/) })
  await view.unmount()
})

test('Stash cancellation is scoped to one queued file and remains retryable', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener('abort', () => reject(new DOMException('canceled', 'AbortError')), { once: true }))
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  const event = new window.Event('drop', { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: { files: [new File(['wait'], 'cancel.txt')] } })
  await act(async () => { dropTarget.dispatchEvent(event) })
  await waitFor(() => assert.ok([...view.container.querySelectorAll('button')].some(button => /^Cancel$/.test(button.textContent || ''))))
  const cancel = [...view.container.querySelectorAll('button')].find(button => /^Cancel$/.test(button.textContent || ''))
  assert.ok(cancel)
  await act(async () => { cancel.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => assert.match(view.container.textContent || '', /cancel\.txt — canceled/))
  assert.ok([...view.container.querySelectorAll('button')].some(button => /retry upload/i.test(button.textContent || '')))
  await view.unmount()
})

test('Stash bounds overlapping upload batches to two workers and rejects overflow without eviction', async () => {
  document.body.replaceChildren()
  let active = 0
  let maxActive = 0
  const started: string[] = []
  const completions: Array<() => void> = []
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    const name = decodeURIComponent(new Headers(init?.headers).get('x-labby-stash-filename') || '')
    started.push(name)
    active += 1
    maxActive = Math.max(maxActive, active)
    await new Promise<void>(resolve => completions.push(resolve))
    active -= 1
    return Response.json({ file_id: name, uri: `stash://me/files/${name}` }, { status: 201 })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  const drop = async (names: string[]) => {
    const event = new window.Event('drop', { bubbles: true, cancelable: true })
    Object.defineProperty(event, 'dataTransfer', { value: { files: names.map(name => new File([name], name)) } })
    await act(async () => { dropTarget.dispatchEvent(event) })
  }

  const accepted = Array.from({ length: 8 }, (_, index) => `accepted-${index}.txt`)
  await drop(accepted)
  await waitFor(() => assert.deepEqual(started, accepted.slice(0, 2)))
  assert.deepEqual(started, accepted.slice(0, 2))
  assert.equal(maxActive, 2)

  await drop(['overflow-a.txt', 'overflow-b.txt'])
  assert.match(view.container.textContent || '', /2 files not queued; the upload queue holds 8/)
  for (const name of accepted) assert.match(view.container.textContent || '', new RegExp(name))
  assert.doesNotMatch(view.container.textContent || '', /overflow-a\.txt|overflow-b\.txt/)

  while (started.length < accepted.length) {
    const ready = completions.splice(0, completions.length)
    await act(async () => { ready.forEach(resolve => resolve()) })
    await waitFor(() => assert.ok(active <= 2))
    assert.ok(active <= 2)
  }
  const ready = completions.splice(0, completions.length)
  await act(async () => { ready.forEach(resolve => resolve()) })
  await waitFor(() => assert.deepEqual(started, accepted))
  assert.equal(maxActive, 2)
  assert.deepEqual(started, accepted)
  await view.unmount()
})

test('Stash prunes terminal history so sequential upload batches remain bounded', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    const name = decodeURIComponent(new Headers(init?.headers).get('x-labby-stash-filename') || '')
    return Response.json({ file_id: name, uri: `stash://me/files/${name}` }, { status: 201 })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))
  assert.ok(dropTarget)
  const drop = async (prefix: string) => {
    const event = new window.Event('drop', { bubbles: true, cancelable: true })
    Object.defineProperty(event, 'dataTransfer', { value: { files: Array.from({ length: 8 }, (_, index) => new File([prefix], `${prefix}-${index}.txt`)) } })
    await act(async () => { dropTarget.dispatchEvent(event) })
    await waitFor(() => assert.match(view.container.textContent || '', new RegExp(`${prefix}-7\\.txt — complete`)))
  }

  await drop('first')
  assert.equal(view.container.querySelector('[aria-label="Upload queue"]')?.children.length, 8)
  assert.match(view.container.textContent || '', /first-0\.txt — complete/)

  await drop('second')
  const queue = view.container.querySelector('[aria-label="Upload queue"]')
  assert.equal(queue?.children.length, 8)
  assert.doesNotMatch(queue?.textContent || '', /first-/)
  assert.match(queue?.textContent || '', /second-0\.txt — complete/)
  assert.match(queue?.textContent || '', /second-7\.txt — complete/)
  await view.unmount()
})

test('same-name uploads from rapid batches keep independent queue identities', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener('abort', () => reject(new DOMException('canceled', 'AbortError')), { once: true }))
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))!
  const drop = () => {
    const event = new window.Event('drop', { bubbles: true, cancelable: true })
    Object.defineProperty(event, 'dataTransfer', { value: { files: [new File(['same'], 'same.txt')] } })
    dropTarget.dispatchEvent(event)
  }
  await act(async () => { drop(); drop() })
  await waitFor(() => assert.equal(view.container.querySelector('[aria-label="Upload queue"]')?.children.length, 2))
  const cancel = [...view.container.querySelectorAll('button')].find(button => /^Cancel$/.test(button.textContent || ''))!
  await act(async () => { cancel.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  await waitFor(() => assert.equal((view.container.textContent || '').match(/same\.txt — canceled/g)?.length, 1))
  assert.equal((view.container.textContent || '').match(/same\.txt — uploading/g)?.length, 1)
  await view.unmount()
})

test('unmount aborts every active upload', async () => {
  document.body.replaceChildren()
  const signals: AbortSignal[] = []
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    signals.push(init?.signal as AbortSignal)
    return new Promise<Response>(() => {})
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))!
  const event = new window.Event('drop', { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: { files: [new File(['a'], 'a.txt'), new File(['b'], 'b.txt')] } })
  await act(async () => { dropTarget.dispatchEvent(event) })
  await waitFor(() => assert.equal(signals.length, 2))
  await view.unmount()
  assert.equal(signals.every(signal => signal.aborted), true)
})

test('upload refresh waits for the entire active queue to become idle', async () => {
  document.body.replaceChildren()
  let statsCalls = 0
  const completions: Array<() => void> = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) {
      statsCalls += 1
      return Response.json({ owned_file_count: statsCalls - 1, owned_shared_file_count: 0, owned_committed_bytes: statsCalls - 1, owned_reserved_bytes: 0 })
    }
    if (!url.pathname.endsWith('/uploads')) return Response.json({ files: [], next_cursor: null })
    await new Promise<void>(resolve => completions.push(resolve))
    return Response.json({ file_id: `uploaded-${completions.length}`, uri: `stash://me/files/uploaded-${completions.length}` }, { status: 201 })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Your Stash is empty/))
  assert.equal(statsCalls, 1)

  const dropTarget = [...view.container.querySelectorAll('button')].find(button => /drop files here/i.test(button.textContent || ''))!
  const event = new window.Event('drop', { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: { files: [new File(['a'], 'a.txt'), new File(['b'], 'b.txt')] } })
  await act(async () => { dropTarget.dispatchEvent(event) })
  await waitFor(() => assert.equal(completions.length, 2))

  await act(async () => { completions[0]() })
  await waitFor(() => assert.match(view.container.textContent || '', /a\.txt — complete/))
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 75)) })
  assert.equal(statsCalls, 1, 'a slow remaining upload must keep the batch refresh deferred')

  await act(async () => { completions[1]() })
  await waitFor(() => assert.equal(statsCalls, 2))
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 75)) })
  assert.equal(statsCalls, 2, 'an idle batch refreshes stats exactly once')
  await view.unmount()
})

test('download intercepts the link, fetches with owner headers, and never puts an owner id in a URL', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', authority: { ...teamAuthority } })
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    if (request.url.endsWith('/stats')) return Response.json({ owned_file_count: 1, owned_shared_file_count: 0, owned_committed_bytes: 5, owned_reserved_bytes: 0 })
    if (request.url.endsWith('/content')) return new Response('hello', { status: 200, headers: { 'content-type': 'text/plain' } })
    return Response.json({ files: [file('report')], next_cursor: null })
  }
  const objectUrls: Blob[] = []
  const clicked: HTMLAnchorElement[] = []
  const previousCreate = URL.createObjectURL
  const previousRevoke = URL.revokeObjectURL
  URL.createObjectURL = (blob: Blob) => { objectUrls.push(blob); return 'blob:labby/download' }
  URL.revokeObjectURL = () => {}
  const previousClick = window.HTMLAnchorElement.prototype.click
  window.HTMLAnchorElement.prototype.click = function (this: HTMLAnchorElement) { clicked.push(this) }
  try {
    const view = await renderClient(<StashPageContent />)
    await waitFor(() => assert.match(view.container.textContent || '', /report\.txt/))
    const link = view.container.querySelector<HTMLAnchorElement>('a[aria-label="Download report.txt"]')
    assert.ok(link)
    assert.equal(link.getAttribute('href'), '/v1/stash/files/report/content')
    assert.equal(link.getAttribute('download'), 'report.txt')
    await act(async () => { link.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true })) })
    await waitFor(() => assert.equal(objectUrls.length, 1))
    assert.equal(await objectUrls[0]!.text(), 'hello')
    const download = requests.find(request => request.url.endsWith('/content'))
    assert.ok(download)
    assert.equal(download.headers.get('x-labby-owner-kind'), 'team')
    assert.equal(download.headers.get('x-labby-owner-id'), 'team-1')
    for (const request of requests) {
      assert.equal(new URL(request.url).search, '')
      assert.doesNotMatch(request.url, /team-1|principal-1/)
    }
    assert.equal(clicked.length, 1, 'the blob anchor is clicked exactly once')
    assert.equal(clicked[0]!.getAttribute('download'), 'report.txt')
    assert.equal(clicked[0]!.getAttribute('href'), 'blob:labby/download')
    await waitFor(() => assert.match(view.container.querySelector('[role="status"]')?.textContent || '', /report\.txt download started/))
    await view.unmount()
  } finally {
    URL.createObjectURL = previousCreate
    URL.revokeObjectURL = previousRevoke
    window.HTMLAnchorElement.prototype.click = previousClick
  }
})

test('a failed download is surfaced through the structured error banner', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', authority: { ...teamAuthority } })
  globalThis.fetch = async (input) => {
    const url = String(input)
    if (url.endsWith('/stats')) return Response.json({ owned_file_count: 1, owned_shared_file_count: 0, owned_committed_bytes: 5, owned_reserved_bytes: 0 })
    if (url.endsWith('/content')) return Response.json({ kind: 'busy', message: 'try later' }, { status: 429 })
    return Response.json({ files: [file('report')], next_cursor: null })
  }
  const view = await renderClient(<StashPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /report\.txt/))
  const link = view.container.querySelector<HTMLAnchorElement>('a[aria-label="Download report.txt"]')
  assert.ok(link)
  await act(async () => { link.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true })) })
  await waitFor(() => assert.match(view.container.querySelector('[role="alert"]')?.textContent || '', /Stash is busy/))
  await view.unmount()
})

test('installation and unbound project workspaces render an explicit unsupported state instead of the personal stash', async () => {
  for (const activeOwner of [{ kind: 'installation' as const, id: 'installation' }, { kind: 'project' as const, id: 'project-1' }]) {
    document.body.replaceChildren()
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', authority: { ...teamAuthority, activeOwner, activeTeamId: undefined, activeProjectId: activeOwner.kind === 'project' ? activeOwner.id : undefined } })
    let fetched = 0
    globalThis.fetch = async () => { fetched += 1; return Response.json({ files: [file('leak')], next_cursor: null }) }
    const view = await renderClient(<StashPageContent />)
    await waitFor(() => assert.match(view.container.querySelector('[role="alert"]')?.textContent || '', /Stash is not available in this workspace/))
    assert.match(view.container.textContent || '', /Switch to a Personal or Team workspace/)
    assert.doesNotMatch(view.container.textContent || '', /leak\.txt/)
    assert.equal(fetched, 0, `no stash request may be sent for the ${activeOwner.kind} workspace`)
    await view.unmount()
  }
})
