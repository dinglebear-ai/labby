import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests, selectSessionWorkspace, type BrowserSessionState } from '../../lib/auth/session-store.ts'
import type { AuthorityProject, AuthoritySnapshot } from '../../lib/auth/authority.ts'
import { filterArtifacts } from './library-model'

const dom = installTestDom()
Object.defineProperty(globalThis, 'NodeFilter', { value: dom.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: dom.HTMLInputElement, configurable: true })
let LibraryPageContent: typeof import('./library-page-content.tsx').LibraryPageContent
test.before(async () => { ({ LibraryPageContent } = await import('./library-page-content.tsx')) })

test('known visibility values render icons and unknown values do not', async () => {
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
    assert.deepEqual([...table.querySelectorAll('th')].map(cell => cell.textContent), ['Kind', 'Artifact', 'Tags', 'Visibility', 'Upstream', 'Updated', 'Open'])
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

test('Library section tabs expose existing routes and reserve unknown count geometry', async () => {
  const { LibraryTabs } = await import('./depot-workspace-pages.tsx')
  const unknown = renderToStaticMarkup(<LibraryTabs active="artifacts" attached />)
  for (const href of ['/library', '/loadouts', '/snippets', '/tools']) assert.ok(unknown.includes(`href="${href}"`))
  assert.equal((unknown.match(/aria-current="page"/g) ?? []).length, 1)
  assert.equal((unknown.match(/tabular-nums/g) ?? []).length, 4)
  assert.equal((unknown.match(/>—<\/span>/g) ?? []).length, 4)
  const known = renderToStaticMarkup(<LibraryTabs active="artifacts" attached counts={{ artifacts: 42 }} />)
  assert.match(known, />42<\/span>/)
  assert.equal((known.match(/tabular-nums/g) ?? []).length, 4)
  assert.equal((known.match(/>—<\/span>/g) ?? []).length, 3)
})
const envelope = (result: unknown) => Response.json(result)
function deferred() {
  let resolve!: (response: Response) => void
  let reject!: (error: Error) => void
  const promise = new Promise<Response>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const flush = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })
const page = (id: string) => <SearchParamsContext.Provider value={new URLSearchParams({ artifact: id })}><LibraryPageContent /></SearchParamsContext.Provider>
const authenticated = (overrides: Partial<Extract<BrowserSessionState, { status: 'authenticated' }>> = {}): BrowserSessionState => ({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', ...overrides })
// The shape the store projects for a source-bound (product-credential) browser
// session: a bound project before any durable authority is attached.
const bindProjectSession = (projectId = 'project-1') => __setBrowserSessionStateForTests(authenticated({ projectId }))
const authority = (projects: AuthorityProject[], activeProjectId?: string): AuthoritySnapshot => ({
  schemaVersion: 1,
  compatibilityGeneration: 1,
  principalId: 'principal-1',
  organizationId: 'org-1',
  activeOwner: activeProjectId ? { kind: 'project', id: activeProjectId } : { kind: 'personal', id: 'principal-1' },
  activeProjectId,
  teams: [],
  projects,
  capabilities: ['scope.read'],
  generation: 1,
})

test('initial catalog load preserves an artifact deep link', async () => {
  const originalFetch = globalThis.fetch
  const originalUrl = window.location.href
  dom.happyDOM.setURL('http://localhost/library/?artifact=alpha')
  bindProjectSession()
  const requested: Array<{ url: string; action: string; params: unknown; projectId: string | null }> = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    requested.push({ url: String(url), action: body.action, params: body.params, projectId: new Headers(init?.headers).get('x-labby-project-id') })
    return envelope(body.action === 'artifacts.get' ? { item: { artifact_id: 'alpha', name: 'Linked artifact', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' } } : { items: [], can_create: false })
  }
  const view = await renderClient(page('alpha'))
  try {
    await flush()
    assert.equal(new URLSearchParams(window.location.search).get('artifact'), 'alpha')
    assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Linked artifact/)
    const deepLink = requested.find(request => request.action === 'artifacts.get')
    assert.ok(deepLink, 'the deep link must resolve through the artifacts control plane')
    assert.equal(deepLink.url, '/v1/artifacts')
    assert.deepEqual(deepLink.params, { artifact_id: 'alpha' })
    assert.equal(deepLink.projectId, 'project-1', 'project-scoped reads carry the bound project')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    dom.happyDOM.setURL(originalUrl)
  }
})

test('detail responses and retained details cannot cross selection or session boundaries', async () => {
  const originalFetch = globalThis.fetch
  bindProjectSession()
  const reads: ReturnType<typeof deferred>[] = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    if (body.action === 'artifacts.get') { const read = deferred(); reads.push(read); return read.promise }
    return envelope({ items: [], can_create: false })
  }
  const view = await renderClient(page('alpha'))
  try {
    await flush()
    await view.rerender(page('bravo'))
    await act(async () => reads[1].resolve(envelope({ item: { artifact_id: 'bravo', name: 'Bravo private details', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' } })))
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
    await act(async () => reads[2].resolve(envelope({ item: { artifact_id: 'charlie', name: 'Charlie private details', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' } })))
    await act(async () => reads[0].resolve(envelope({ item: { artifact_id: 'alpha', name: 'Alpha stale details', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' } })))
    assert.doesNotMatch(document.body.textContent ?? '', /Alpha stale details/)
    bindProjectSession('project-2')
    await view.rerender(page('charlie'))
    assert.doesNotMatch(document.body.textContent ?? '', /Charlie private details/)
    await flush()
    assert.equal(reads.length, 4, 'a new project context issues its own read')
    await act(async () => reads[3].reject(new Error('new session denied')))
    assert.doesNotMatch(document.body.textContent ?? '', /Bravo private details|Charlie private details|Alpha stale details/)
    assert.match(document.body.textContent ?? '', /Artifact details are unavailable/)
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await view.rerender(page('charlie'))
    await flush()
    assert.equal(reads.length, 4, 'a session without a project issues no project-scoped read')
    assert.equal(document.querySelector('[role="dialog"]'), null, 'no retained detail survives the session')
    assert.doesNotMatch(document.body.textContent ?? '', /Bravo private details|Charlie private details|Alpha stale details|Artifact details are unavailable/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('late page failures and successes cannot overwrite a new query', async () => {
  const originalFetch = globalThis.fetch
  bindProjectSession()
  const pending: ReturnType<typeof deferred>[] = []
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/status') return Response.json({ depot: { configured: true, enabled: true, maxResponseBytes: 10000 } })
    if (url === '/v1/depot/publish') return Response.json({ available: false })
    const body = JSON.parse(String(init?.body))
    if (body.params.cursor) { const read = deferred(); pending.push(read); return read.promise }
    return envelope({ items: [{ artifact_id: body.params.query || 'initial', name: body.params.query || 'Initial item', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' }], next_cursor: 'next', can_create: false })
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
    await act(async () => pending[0].resolve(envelope({ items: [{ artifact_id: 'stale', name: 'Stale page', latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' }], can_create: false })))
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

// Page-level tests against a fake fetch follow.
import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'

const router = { push: () => {}, replace: () => {}, back: () => {}, forward: () => {}, refresh: () => {}, prefetch: () => {} }

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

const WRITE_DEPOT = { configured: true, enabled: true, authority: 'write', can_create: true, maxResponseBytes: 1_048_576 }
type LibraryRequest = { path: string; projectId: string | null }

async function renderLibrary(
  depot: Record<string, unknown>,
  session: BrowserSessionState = authenticated({ projectId: 'project-1' }),
  artifacts: (projectId: string | null) => unknown[] = () => [],
) {
  __setBrowserSessionStateForTests(session)
  const requested: LibraryRequest[] = []
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input), 'http://labby.test').pathname
    const projectId = new Headers(init?.headers).get('x-labby-project-id')
    requested.push({ path, projectId })
    if (path === '/v1/depot/status') return Response.json({ depot })
    if (path === '/v1/depot/publish') return Response.json({ available: depot.enabled === true && depot.authority === 'write' })
    if (depot.error) return Response.json({ message: depot.error }, { status: 403 })
    const items = artifacts(projectId)
    return Response.json({ items, can_create: depot.can_create === true })
  }) as typeof globalThis.fetch
  document.body.replaceChildren()
  // A fresh element per render: React bails out of an identical element, so a
  // rerender that must observe a session set through the test hook (which
  // notifies no subscriber) needs new props.
  const element = () => (
    <AppRouterContext.Provider value={router as never}>
      <PathnameContext.Provider value="/library">
        <SearchParamsContext.Provider value={new URLSearchParams() as never}>
          <LibraryPageContent />
        </SearchParamsContext.Provider>
      </PathnameContext.Provider>
    </AppRouterContext.Provider>
  )
  const view = await renderClient(element())
  return { view, element, requested }
}

test('Library uses local acquired records and access without a remote Depot dependency', async () => {
  const local = await renderLibrary({ can_create: true })
  await waitFor(() => assert.equal(local.view.container.querySelector('[data-library-access]')?.textContent, 'Read + create'))
  assert.match(local.view.container.textContent ?? '', /Local acquired artifacts/)
  assert.equal(local.requested.some(request => request.path.startsWith('/v1/depot')), false)
  await local.view.unmount()
  const readOnly = await renderLibrary({ can_create: false })
  await waitFor(() => assert.equal(readOnly.view.container.querySelector('[data-library-access]')?.textContent, 'Read only'))
  await readOnly.view.unmount()
})

test('missing project context directs the operator to the existing workspace selector', async () => {
  const missing = await renderLibrary({ error: 'Skill Library project context is required' })
  await waitFor(() => assert.match(missing.view.container.textContent ?? '', /Choose a project in the workspace selector/))
  assert.match(missing.view.container.textContent ?? '', /Project access is required/)
  await missing.view.unmount()
})

/**
 * The server refuses every `artifacts.*` request that arrives without a
 * project, and an OAuth or bearer sign-in starts in the Personal workspace
 * with none. The page therefore issues no request at all until a project is
 * chosen, offers the projects the server projected, and keeps the Library
 * shell navigable meanwhile.
 */
test('the Personal workspace offers the eligible projects before any request', async () => {
  const { view, requested } = await renderLibrary(WRITE_DEPOT, authenticated({ authority: authority([{ id: 'project-1', role: 'owner', name: 'Project One' }]) }))
  try {
    await flush()
    assert.equal(requested.length, 0, 'no request may run before a project is selected')
    assert.match(view.container.textContent ?? '', /Project required/)
    assert.doesNotMatch(view.container.textContent ?? '', /Depot unavailable/)
    assert.ok(view.container.querySelector('a[href="/loadouts"]'), 'the other Library sections stay reachable')
    const choose = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Project One'))
    assert.ok(choose, 'the server-projected project is offered')
    await act(async () => choose.click())
    await waitFor(() => assert.ok(requested.some(request => request.path === '/v1/artifacts')))
    assert.ok(requested.filter(request => request.path === '/v1/artifacts').every(request => request.projectId === 'project-1'), 'every artifact read carries the selected project')
    await waitFor(() => assert.doesNotMatch(view.container.textContent ?? '', /Project required/))
  } finally { await view.unmount() }
})

test('a session with no eligible project explains the gap instead of issuing reads that must fail', async () => {
  const { view, requested } = await renderLibrary(WRITE_DEPOT, authenticated({ authority: authority([]) }))
  try {
    await flush()
    assert.equal(requested.length, 0, 'no request may run without an eligible project')
    assert.match(view.container.textContent ?? '', /Project required/)
    assert.match(view.container.textContent ?? '', /No eligible project is available/)
    assert.equal(view.container.querySelector('[role="alert"]'), null)
  } finally { await view.unmount() }
})

test('switching projects through the shared workspace switch remounts the collection under the new project', async () => {
  const projects: AuthorityProject[] = [{ id: 'project-1', role: 'owner' }, { id: 'project-2', role: 'owner' }]
  const { view, requested } = await renderLibrary(
    WRITE_DEPOT,
    authenticated({ projectId: 'project-1', authority: authority(projects, 'project-1') }),
    projectId => [{ artifact_id: `${projectId}-artifact`, name: `${projectId} artifact`, latest_revision_id: 'revision-one', latest_revision_files: [], access_label: 'private', visibility: 'private' }],
  )
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /project-1 artifact/))
    await act(async () => { selectSessionWorkspace({ projectId: 'project-2' }) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /project-2 artifact/))
    assert.doesNotMatch(view.container.textContent ?? '', /project-1 artifact/)
    const reads = requested.filter(request => request.path === '/v1/artifacts').map(request => request.projectId)
    assert.deepEqual([...new Set(reads)], ['project-1', 'project-2'], 'the remounted collection reads under the newly selected project')
  } finally { await view.unmount() }
})

test('mock data mode mounts the preview collection without a session', async () => {
  const previous = process.env.NEXT_PUBLIC_MOCK_DATA
  process.env.NEXT_PUBLIC_MOCK_DATA = 'true'
  const { view, requested } = await renderLibrary(WRITE_DEPOT, { status: 'loading' })
  try {
    await waitFor(() => assert.ok(requested.some(request => request.path === '/v1/artifacts')))
    assert.ok(view.container.querySelector('input[aria-label="Search library"]'), 'the collection renders against the mock endpoints')
    assert.doesNotMatch(view.container.textContent ?? '', /Project required/)
  } finally {
    await view.unmount()
    if (previous === undefined) delete process.env.NEXT_PUBLIC_MOCK_DATA
    else process.env.NEXT_PUBLIC_MOCK_DATA = previous
  }
})

test('a session that has not resolved issues no request and mounts once it is bound', async () => {
  const { view, element, requested } = await renderLibrary(WRITE_DEPOT, { status: 'loading' })
  try {
    await flush()
    assert.equal(requested.length, 0, 'an unresolved session issues no request')
    assert.ok(view.container.querySelector('a[href="/loadouts"]'), 'the Library shell renders while the session resolves')
    bindProjectSession()
    await view.rerender(element())
    await waitFor(() => assert.ok(requested.some(request => request.path === '/v1/artifacts' && request.projectId === 'project-1')))
  } finally { await view.unmount() }
})
