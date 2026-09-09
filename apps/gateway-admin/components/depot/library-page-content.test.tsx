import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
import { filterArtifacts } from './library-model'

const dom = installTestDom()
Object.defineProperty(globalThis, 'NodeFilter', { value: dom.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: dom.HTMLInputElement, configurable: true })
let LibraryPageContent: typeof import('./library-page-content.tsx').LibraryPageContent
test.before(async () => { ({ LibraryPageContent } = await import('./library-page-content.tsx')) })

test('visibility uses reference icons only for explicitly reported known values', async () => {
  const { LibraryVisibility } = await import('./library-page-content.tsx')
  for (const [visibility, icon] of [['public', 'globe'], ['team', 'users'], ['private', 'lock-keyhole']]) {
    const html = renderToStaticMarkup(<LibraryVisibility visibility={visibility} />)
    assert.match(html, new RegExp(`lucide-${icon}`))
    assert.match(html, /aria-hidden="true"/)
  }
  for (const visibility of [undefined, 'future-policy', '__proto__', 'constructor']) {
    const html = renderToStaticMarkup(<LibraryVisibility visibility={visibility} />)
    assert.doesNotMatch(html, /<svg/)
    assert.ok(html.includes(visibility || 'Unknown'))
  }
})
test('every loaded extended kind is selectable and filters its actual artifacts', async () => {
  const { LibraryFilterRail, libraryFilterKinds } = await import('./library-page-content.tsx')
  const kinds = ['hook', 'extension', 'loadout', 'snippet']
  const artifacts = kinds.map(kind => ({ id: `${kind}-one`, kind }))
  const categories = libraryFilterKinds([...artifacts, ...artifacts])
  for (const kind of kinds) assert.equal(categories.filter(value => value === kind).length, 1)
  let visible = artifacts
  const view = await renderClient(<LibraryFilterRail kind="all" artifacts={artifacts} onKind={kind => { visible = filterArtifacts(artifacts, kind) as typeof artifacts }} />)
  try {
    for (const kind of kinds) {
      const label = kind.charAt(0).toUpperCase() + kind.slice(1) + 's'
      const button = [...document.querySelectorAll<HTMLButtonElement>('[data-lbrail] button')].find(button => button.textContent === `${label}1`)
      assert.ok(button, `${kind} is present with its loaded count`)
      assert.ok(button.querySelector('svg'))
      await act(async () => button.click())
      assert.deepEqual(visible.map(artifact => artifact.id), [`${kind}-one`])
    }
  } finally { await view.unmount() }
})
test('compact collection table preserves real metadata and single inspection activation', async () => {
  const { LibraryArtifactTable } = await import('./library-page-content.tsx')
  const selected: string[] = []
  const view = await renderClient(<LibraryArtifactTable artifacts={[{ id: 'one', title: 'Real artifact', kind: 'hook', namespace: 'actual-owner', descriptor: { tags: ['automation', 'verified-source'] }, publication: { visibility: 'public' } }]} onInspect={id => selected.push(id)}/>)
  try {
    const table = document.querySelector('table')!
    assert.equal(table.getAttribute('aria-label'), 'Library artifacts')
    assert.match(table.className, /table-fixed/)
    assert.match(table.parentElement!.className, /max-h-\[56vh\]/)
    assert.deepEqual([...table.querySelectorAll('th')].map(cell => cell.textContent), ['Kind', 'Artifact', 'Tags', 'Visibility', 'Open'])
    assert.ok(table.querySelector('[title="actual-owner"]'))
    assert.match(table.textContent ?? '', /automation/)
    assert.match(table.textContent ?? '', /verified-source/)
    assert.match(table.textContent ?? '', /Public/)
    const inspect = table.querySelector<HTMLButtonElement>('button')!
    assert.equal(inspect.getAttribute('aria-label'), 'Inspect Real artifact')
    await act(async () => inspect.click())
    assert.deepEqual(selected, ['one'])
    await act(async () => table.querySelector<HTMLTableCellElement>('tbody td')!.click())
    assert.deepEqual(selected, ['one', 'one'])
  } finally { await view.unmount() }
})

test('Library rail uses real loaded kinds and delegates filtering without claiming catalog-wide counts', async () => {
  const { LibraryFilterRail } = await import('./library-page-content.tsx')
  const selected: string[] = []
  const view = await renderClient(<LibraryFilterRail kind="all" artifacts={[{ id: 'one', kind: 'skill' }, { id: 'two', kind: 'agent' }]} onKind={kind => selected.push(kind)} />)
  try {
    assert.match(document.body.textContent ?? '', /loaded results/)
    assert.match(document.body.textContent ?? '', /No tags supplied in loaded results/)
    const buttons = [...document.querySelectorAll<HTMLButtonElement>('[data-lbrail] button')]
    assert.equal(buttons.filter(button => button.getAttribute('aria-pressed') === 'true').length, 1)
    const skill = buttons.find(button => /skill/i.test(button.textContent ?? ''))!
    await act(async () => skill.click())
    assert.deepEqual(selected, ['skill'])
    assert.match(skill.textContent ?? '', /1$/)
  } finally { await view.unmount() }
})
test('narrow Library filters collapse without losing the selected kind or tag', async () => {
  const { LibraryFilterRail } = await import('./library-page-content.tsx')
  const view = await renderClient(<LibraryFilterRail artifacts={[]} kind="skill" tag="automation" onKind={() => {}}/>)
  try {
    const toggle = document.querySelector<HTMLButtonElement>('[data-lbrail] button[aria-expanded]')!
    const content = document.getElementById(toggle.getAttribute('aria-controls')!)!
    assert.equal(toggle.getAttribute('aria-expanded'), 'false')
    assert.match(toggle.textContent ?? '', /Skills · automation/)
    assert.ok(content.classList.contains('hidden'))
    assert.ok(content.classList.contains('min-[901px]:block'))
    await act(async () => toggle.click())
    assert.equal(toggle.getAttribute('aria-expanded'), 'true')
    assert.ok(!content.classList.contains('hidden'))
    await act(async () => toggle.click())
    assert.equal(toggle.getAttribute('aria-expanded'), 'false')
    assert.match(toggle.textContent ?? '', /Skills · automation/)
  } finally { await view.unmount() }
})
test('tag rail shows supplied loaded counts and toggles the selected tag', async () => {
  const { LibraryFilterRail } = await import('./library-page-content.tsx')
  const selected: Array<string | undefined> = []
  const artifacts = [{ id: 'one', descriptor: { tags: ['automation'] } }, { id: 'two', descriptor: { tags: ['automation', 'tools'] } }]
  const view = await renderClient(<LibraryFilterRail artifacts={artifacts} kind="all" onKind={() => {}} tag="automation" onTag={tag => selected.push(tag)}/>)
  try {
    const tags = document.querySelector('section[aria-label="Tags"]')!
    assert.match(tags.textContent ?? '', /automation2/)
    assert.match(tags.textContent ?? '', /tools1/)
    const active = tags.querySelector<HTMLButtonElement>('[aria-pressed="true"]')!
    await act(async () => active.click())
    assert.deepEqual(selected, [undefined])
    await act(async () => [...tags.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'tools1')!.click())
    assert.deepEqual(selected, [undefined, 'tools'])
  } finally { await view.unmount() }
})

test('sort menu exposes only supported loaded-result orders', async () => {
  const { LibrarySortMenu } = await import('./library-page-content.tsx')
  const selected: string[] = []
  const view = await renderClient(<LibrarySortMenu sort="catalog" onSort={sort => selected.push(sort)}/>)
  try {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Sort loaded library results"]')!.click())
    assert.match(document.body.textContent ?? '', /Sort loaded results/)
    for (const label of ['Name', 'Kind']) {
      const button = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === label)!
      assert.ok(button)
      await act(async () => button.click())
    }
    assert.deepEqual(selected, ['name', 'kind'])
    assert.doesNotMatch(document.body.textContent ?? '', /Updated|Newest/)
  } finally { await view.unmount() }
})

test('Library navigation exposes existing routes and only authoritative counts', async () => {
  const { LibraryNavigation } = await import('./library-page-content.tsx')
  const unknown = renderToStaticMarkup(<LibraryNavigation />)
  for (const href of ['/library', '/loadouts', '/snippets', '/tools']) assert.ok(unknown.includes(`href="${href}"`))
  assert.equal((unknown.match(/aria-current="page"/g) ?? []).length, 1)
  assert.doesNotMatch(unknown, /tabular-nums/)
  const known = renderToStaticMarkup(<LibraryNavigation total={42} />)
  assert.match(known, />42<\/span>/)
})
const envelope = (result: unknown) => Response.json({ schemaVersion: 'labby.depot-compatibility/v1', result })
function deferred() {
  let resolve!: (response: Response) => void
  let reject!: (error: Error) => void
  const promise = new Promise<Response>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const flush = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })
const page = (id: string) => <SearchParamsContext.Provider value={new URLSearchParams({ artifact: id })}><LibraryPageContent /></SearchParamsContext.Provider>

test('initial catalog load preserves an artifact deep link', async () => {
  const originalFetch = globalThis.fetch
  const originalUrl = window.location.href
  dom.happyDOM.setURL('http://localhost/library/?artifact=alpha')
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    return envelope(body.operation === 'depot.artifacts.get' ? { artifact: { id: 'alpha', title: 'Linked artifact' } } : { artifacts: [] })
  }
  const view = await renderClient(page('alpha'))
  try {
    await flush()
    assert.equal(new URLSearchParams(window.location.search).get('artifact'), 'alpha')
    assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Linked artifact/)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    dom.happyDOM.setURL(originalUrl)
  }
})

test('detail responses and retained details cannot cross selection or session boundaries', async () => {
  const originalFetch = globalThis.fetch
  const reads: ReturnType<typeof deferred>[] = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    if (body.operation === 'depot.artifacts.get') { const read = deferred(); reads.push(read); return read.promise }
    return envelope({ artifacts: [] })
  }
  const view = await renderClient(page('alpha'))
  try {
    await flush()
    await view.rerender(page('bravo'))
    await act(async () => reads[1].resolve(envelope({ artifact: { id: 'bravo', title: 'Bravo private details' } })))
    assert.match(document.body.textContent ?? '', /Bravo private details/)
    const dialog = document.querySelector('[role="dialog"]')!
    assert.match(dialog.className, /max-w-\[720px\]/)
    assert.match(dialog.className, /max-h-\[86vh\]/)
    const source = dialog.querySelector('details')!
    assert.equal(source.open, false)
    assert.match(source.textContent ?? '', /Artifact ID/)
    assert.ok(dialog.querySelector('button[aria-label="Export JSON"]'))
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
