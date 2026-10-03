import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { filterArtifacts } from './library-model'

const dom = installTestDom()
test.after(() => dom.happyDOM.close())
Object.defineProperty(globalThis, 'self', { value: globalThis.window, configurable: true })
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
    assert.match(table.parentElement!.className, /overflow-auto/)
    assert.equal(table.parentElement!.style.maxHeight, '55.3vh')
    assert.deepEqual([...table.querySelectorAll('th')].map(cell => cell.textContent), ['Kind', 'Artifact', 'Tags', 'Visibility', 'Upstream', 'Updated', 'Actions'])
    assert.ok(table.querySelector('[title="actual-owner"]'))
    assert.match(table.textContent ?? '', /automation/)
    assert.match(table.textContent ?? '', /verified-source/)
    assert.match(table.textContent ?? '', /Public/)
    // The row now leads with a selection control; find the inspect action by name.
    const inspect = table.querySelector<HTMLButtonElement>('button[aria-label="Inspect Real artifact"]')!
    assert.ok(inspect)
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
test('narrow Library filters stay out of the layout until the toolbar opens them', async () => {
  const { LibraryFilterRail } = await import('./library-page-content.tsx')
  const view = await renderClient(<LibraryFilterRail artifacts={[]} kind="skill" tag="automation" onKind={() => {}}/>)
  try {
    const rail = document.querySelector<HTMLElement>('[data-lbrail]')!
    assert.ok(rail.classList.contains('hidden'))
    assert.ok(rail.classList.contains('min-[901px]:block'))
    assert.equal(rail.querySelector('button[aria-expanded]'), null)
  } finally { await view.unmount() }
  const open = await renderClient(<LibraryFilterRail artifacts={[]} kind="skill" tag="automation" mobileOpen onKind={() => {}}/>)
  try { assert.ok(document.querySelector<HTMLElement>('[data-lbrail]')!.classList.contains('block')) }
  finally { await open.unmount() }
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
    await act(async () => [...tags.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === '#tools1')!.click())
    assert.deepEqual(selected, [undefined, 'tools'])
  } finally { await view.unmount() }
})

test('sort menu exposes only supported loaded-result orders', async () => {
  const { LibrarySortMenu } = await import('./library-page-content.tsx')
  const selected: string[] = []
  const view = await renderClient(<LibrarySortMenu sort="catalog" onSort={sort => selected.push(sort)}/>)
  try {
    const group = document.querySelector('[aria-label="Sort loaded library results"]')!
    const buttons = [...group.querySelectorAll<HTMLButtonElement>('button')]
    assert.deepEqual(buttons.map(button => button.textContent), ['Updated', 'Name', 'Kind'])
    assert.equal(buttons[0].getAttribute('aria-pressed'), 'true')
    await act(async () => buttons[1].click())
    await act(async () => buttons[2].click())
    assert.deepEqual(selected, ['name', 'kind'])
  } finally { await view.unmount() }
})

test('Library hub matches the four primary mock tabs with stable icon/count geometry', async () => {
  const { LibraryTabs } = await import('./depot-workspace-pages.tsx')
  const unknown = renderToStaticMarkup(<LibraryTabs active="artifacts" attached />)
  const hrefs = ['/library', '/loadouts', '/snippets', '/tools']
  for (const href of hrefs) assert.ok(unknown.includes(`href="${href}"`), href)
  for (const label of ['Artifacts', 'Loadouts', 'Snippets', 'Tools']) assert.match(unknown, new RegExp(`>${label}<`))
  assert.equal((unknown.match(/aria-current="page"/g) ?? []).length, 1)
  assert.equal((unknown.match(/tabular-nums/g) ?? []).length, 4)
  assert.equal((unknown.match(/>—<\/span>/g) ?? []).length, 4)
  assert.ok((unknown.match(/<svg/g) ?? []).length >= 4)
  const known = renderToStaticMarkup(<LibraryTabs active="artifacts" attached counts={{ artifacts: 16, loadouts: 3, snippets: 6, tools: 74 }} />)
  for (const value of ['16', '3', '6', '74']) assert.match(known, new RegExp(`>${value}<`))
})
const DEPOT_SCHEMA = 'labby.depot-compatibility/v1'
const envelope = (result: unknown) => Response.json({ schemaVersion: DEPOT_SCHEMA, result })
const operationCatalog = () => Response.json({ operations: [
  { name: 'depot.artifacts.list', title: 'List artifacts', description: 'List artifacts', inputSchema: { type: 'object', properties: {}, additionalProperties: false }, annotations: { readOnlyHint: true, destructiveHint: false } },
  { name: 'depot.artifacts.get', title: 'Get artifact', description: 'Get artifact', inputSchema: { type: 'object', properties: {}, additionalProperties: false }, annotations: { readOnlyHint: true, destructiveHint: false } },
] })
const flush = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })

test('production Library displays real revision contents and exact upstream identity', async () => {
  const record = { ...artifact('unique-skill', 'skill'), providerId: 'team-catalog',
    readme: { state: 'available' as const, kind: 'readme' as const, path: 'README.md' as const, revisionId: 'unique-skill-rev', content: '# Actual revision\n\nREADME_MARKER' },
    lineage: { upstreamArtifactId: 'original', following: true },
  }
  const { view, restore } = await renderLibrary(new URLSearchParams({ artifact: record.id }), [record])
  try {
    await flush()
    const dialog = document.querySelector('[role="dialog"]')!
    assert.match(dialog.textContent ?? '', /README_MARKER/)
    assert.doesNotMatch(dialog.textContent ?? '', /Forks stay linked|reference.md|examples\/basic.md/)
    await act(async () => dialog.querySelector<HTMLButtonElement>('button[aria-expanded]')!.click())
    assert.match(dialog.textContent ?? '', /README.md/)
    assert.doesNotMatch(dialog.textContent ?? '', /SKILL.md|scripts\/unique-skill.sh/)
    const destination = new URL(dialog.querySelector<HTMLAnchorElement>('[aria-label="Upstream"] a')!.getAttribute('href')!, 'https://labby.example')
    assert.equal(destination.searchParams.get('artifactProvider'), 'team-catalog')
    assert.equal(destination.searchParams.get('artifact'), 'original')
    assert.equal(dialog.querySelector('button[title="Add to Library"]'), null)
  } finally { await view.unmount(); restore() }
})

test('same-route navigation replaces stale search and kind filters', async () => {
  const records = [artifact('alpha', 'skill', 'Alpha Skill'), artifact('beta', 'plugin', 'Beta Plugin')]
  const { view, requested, restore } = await renderLibrary(new URLSearchParams({ q: 'Alpha', kind: 'skill' }), records)
  try {
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 360)) })
    assert.match(view.container.textContent ?? '', /Alpha Skill/)
    await view.rerender(<SearchParamsContext.Provider value={new URLSearchParams({ q: 'Beta', kind: 'plugin', artifact: 'beta' }) as never}><LibraryPageContent /></SearchParamsContext.Provider>)
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 360)) })
    assert.equal(view.container.querySelector<HTMLInputElement>('input[aria-label="Search library"]')?.value, 'Beta')
    assert.match(view.container.textContent ?? '', /Beta Plugin/)
    assert.doesNotMatch(view.container.textContent ?? '', /Alpha Skill/)
    assert.equal(requested.filter(request => request.operation === 'depot.artifacts.list').at(-1)?.input.query, 'Beta')
    assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Beta Plugin/)
  } finally { await view.unmount(); restore() }
})

test('denied clipboard writes surface recovery without an unhandled rejection', async () => {
  const { toast } = await import('sonner')
  const originalError = toast.error
  const messages: string[] = []
  toast.error = (message => { messages.push(String(message)); return 'test' }) as typeof toast.error
  const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('Permission denied') } } })
  const { view, restore } = await renderLibrary(new URLSearchParams({ artifact: 'skill-one' }))
  try {
    await flush()
    await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Copy Library link"]')!.click())
    assert.deepEqual(messages, ['Could not copy share link. Allow clipboard access and try again.'])
  } finally {
    await view.unmount(); restore(); toast.error = originalError
    if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})

function artifact(id: string, kind: string, title = id) {
  return {
    id, kind, namespace: 'tootie.tv', name: id, title, description: title + ' description', revisionCount: 1,
    descriptor: { id, kind, namespace: 'tootie.tv', name: id, title, description: title + ' description', tags: ['mock'] },
    currentRevision: { id: id + '-rev', contentDigest: 'sha256:' + id, createdAt: '2026-09-16T00:00:00Z', components: [{ id: id + '-file', kind: 'document', path: 'README.md', mediaType: 'text/markdown', size: 24 }] },
    publication: { state: 'published', visibility: 'public', distribution: 'catalog' },
  }
}

test('late clipboard settlement after unmount cannot publish feedback', async () => {
  const { toast } = await import('sonner')
  const originalSuccess = toast.success, originalError = toast.error
  const feedback: string[] = []
  toast.success = (message => { feedback.push(String(message)); return 'test' }) as typeof toast.success
  toast.error = (message => { feedback.push(String(message)); return 'test' }) as typeof toast.error
  const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  try {
    for (const fail of [false, true]) {
      let resolve!: () => void, reject!: (error: Error) => void
      const pending = new Promise<void>((yes, no) => { resolve = yes; reject = no })
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: () => pending } })
      const { view, restore } = await renderLibrary(new URLSearchParams({ artifact: 'skill-one' }))
      try {
        await flush()
        await act(async () => document.querySelector<HTMLButtonElement>('button[aria-label="Copy Library link"]')!.click())
        await view.unmount()
        await act(async () => { if (fail) reject(new Error('denied')); else resolve() })
      } finally { restore() }
    }
    assert.deepEqual(feedback, [])
  } finally {
    toast.success = originalSuccess; toast.error = originalError
    if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})

type DepotRequest = { operation: string; input: Record<string, unknown>; projectId: string | null }

async function renderLibrary(search = new URLSearchParams(), records = [artifact('skill-one', 'skill', 'Skill One')], pages?: ReturnType<typeof artifact>[][], omitTotal = false) {
  const requested: DepotRequest[] = []
  const requestSequence: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = (init?.method ?? 'GET').toUpperCase()
    if (String(input) === '/v1/depot/operations' && method === 'GET') {
      requestSequence.push('catalog')
      return operationCatalog()
    }
    const headers = new Headers(init?.headers)
    const body = JSON.parse(String(init?.body ?? '{}')) as { operation?: string; params?: Record<string, unknown> }
    if (body.operation) {
      requestSequence.push(body.operation)
      requested.push({ operation: body.operation, input: body.params ?? {}, projectId: headers.get('x-labby-project-id') })
    }
    if (body.operation === 'depot.artifacts.get') {
      const id = String(body.params?.artifactId ?? '')
      return envelope({ artifact: records.find(item => item.id === id) ?? artifact(id, 'skill', 'Deep linked artifact') })
    }
    if (body.operation === 'depot.artifacts.list') {
      const page = Number(body.params?.cursor ?? 0)
      return envelope({ artifacts: pages?.[page] ?? records, ...(omitTotal ? {} : { total: pages ? pages.reduce((sum, items) => sum + items.length, 0) : records.length }), ...(pages && page + 1 < pages.length ? { nextCursor: String(page + 1) } : {}) })
    }
    return Response.json({ message: 'unexpected request' }, { status: 500 })
  }) as typeof globalThis.fetch
  document.body.replaceChildren()
  const view = await renderClient(<SearchParamsContext.Provider value={search as never}><LibraryPageContent /></SearchParamsContext.Provider>)
  return { view, requested, requestSequence, restore: () => { globalThis.fetch = originalFetch } }
}

test('Library is a user-level hub and loads the generic Depot Artifact authority without a project gate', async () => {
  const records = [
    artifact('prompt-one', 'prompt', 'Prompt One'), artifact('resource-one', 'resource', 'Resource One'), artifact('app-one', 'app', 'App One'),
    artifact('skill-one', 'skill', 'Skill One'), artifact('plugin-one', 'plugin', 'Plugin One'), artifact('market-one', 'marketplace', 'Marketplace One'),
  ]
  const { view, requested, requestSequence, restore } = await renderLibrary(new URLSearchParams(), records)
  try {
    await flush()
    const heroMain = view.container.querySelector<HTMLElement>('[data-console-hero-main="1"]')
    const heroTitle = view.container.querySelector<HTMLElement>('[data-console-hero-title="1"]')
    const heroStats = view.container.querySelector<HTMLElement>('[data-console-hero-stats="1"]')
    assert.ok(heroMain)
    assert.equal(heroTitle?.textContent, 'Library')
    assert.ok(heroStats)
    assert.ok(view.container.querySelector('[data-console-hero-actions-mixed="1"]'), 'Library keeps its labeled New Loadout action visible without restoring the oversized hero')
    assert.equal(view.container.querySelector('[data-console-hero-actions="1"]'), null)
    assert.match(view.container.querySelector('button[aria-label="Export loaded library metadata"]')?.className ?? '', /size-9/)
    assert.match(view.container.querySelector('button[aria-label="New loadout"]')?.className ?? '', /h-9/)
    assert.doesNotMatch(view.container.textContent ?? '', /Project required|Select an eligible project workspace/)
    assert.ok(requested.some(request => request.operation === 'depot.artifacts.list'))
    const listIndex = requestSequence.indexOf('depot.artifacts.list')
    assert.ok(listIndex > 0, 'artifact listing was dispatched')
    assert.ok(requestSequence.slice(0, listIndex).includes('catalog'), 'Library establishes the actor-filtered Depot catalog before listing artifacts')
    assert.ok(requested.filter(request => request.operation === 'depot.artifacts.list').every(request => request.projectId === null), 'Library browsing does not require a project header')
    for (const title of records.map(item => item.title)) assert.match(view.container.textContent ?? '', new RegExp(title))
    assert.ok(view.container.querySelector('a[href="/library"][aria-current="page"]'))
    for (const href of ['/loadouts', '/snippets', '/tools']) assert.ok(view.container.querySelector(`a[href="${href}"]`))
  } finally { await view.unmount(); restore() }
})

test('Library fails closed when the actor-filtered Depot catalog cannot be established', async () => {
  const originalFetch = globalThis.fetch
  let operationPosts = 0
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input) === '/v1/depot/operations' && (init?.method ?? 'GET').toUpperCase() === 'GET') {
      return new Response('<html>bad gateway</html>', { status: 502, headers: { 'content-type': 'text/html' } })
    }
    if (String(input) === '/v1/depot/operations' && (init?.method ?? 'GET').toUpperCase() === 'POST') operationPosts++
    return envelope({ artifacts: [], total: 0 })
  }) as typeof globalThis.fetch
  document.body.replaceChildren()
  const view = await renderClient(<SearchParamsContext.Provider value={new URLSearchParams() as never}><LibraryPageContent /></SearchParamsContext.Provider>)
  try {
    await flush()
    assert.equal(operationPosts, 0, 'Library never dispatches an operation without a current catalog')
    assert.match(view.container.textContent ?? '', /Library unavailable/i)
    assert.ok(view.container.querySelector('button') && [...view.container.querySelectorAll('button')].some(button => button.textContent?.includes('Retry loading')))
    assert.doesNotMatch(view.container.textContent ?? '', /No artifacts in your library yet/i)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('Library kind routes use one hub surface and activate the requested family', async () => {
  const params = new URLSearchParams({ kind: 'resource' })
  const { view, restore } = await renderLibrary(params, [artifact('r1', 'resource', 'Docs Resource'), artifact('s1', 'skill', 'Hidden Skill')])
  try {
    await flush()
    assert.match(view.container.textContent ?? '', /Docs Resource/)
    assert.doesNotMatch(view.container.textContent ?? '', /Hidden Skill/)
    assert.ok(view.container.querySelector('a[href="/library"][aria-current="page"]'))
    assert.equal(view.container.querySelector('a[href="/library?kind=resource"]'), null)
  } finally { await view.unmount(); restore() }
})

test('Library deep links open the shared mock-aligned inspection modal with icon-only actions', async () => {
  const params = new URLSearchParams({ artifact: 'rust-reviewer' })
  const record = artifact('rust-reviewer', 'agent', 'rust-reviewer')
  record.descriptor.tags = ['rust', 'review']
  const { view, requested, requestSequence, restore } = await renderLibrary(params, [record])
  try {
    await flush()
    const dialog = document.querySelector('[role="dialog"]')
    assert.ok(dialog)
    assert.match(dialog.textContent ?? '', /rust-reviewer/)
    assert.ok(requested.some(request => request.operation === 'depot.artifacts.get' && request.input.artifactId === 'rust-reviewer'))
    const getIndex = requestSequence.indexOf('depot.artifacts.get')
    assert.ok(getIndex > 0, 'artifact detail was dispatched')
    assert.ok(requestSequence.slice(0, getIndex).includes('catalog'), 'Library establishes the actor-filtered Depot catalog before artifact detail calls')
    for (const label of ['Open upstream and fork options', 'Copy Library link', 'Export artifact']) assert.ok(dialog.querySelector(`button[aria-label="${label}"]`), label)
    assert.doesNotMatch(dialog.textContent ?? '', /Open upstream and fork options|Copy Library link|Export artifact/)
  } finally { await view.unmount(); restore() }
})

test('Library search sends supported semantic queries but still filters loaded results locally', async () => {
  const { view, requested, restore } = await renderLibrary(new URLSearchParams(), [artifact('alpha', 'skill', 'Alpha Skill'), artifact('beta', 'plugin', 'Beta Plugin')])
  try {
    await flush()
    const input = view.container.querySelector<HTMLInputElement>('input[aria-label="Search library"]')!
    const key = Object.keys(input).find(key => key.startsWith('__reactProps$'))!
    const props = (input as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[key]
    await act(async () => props.onChange({ target: { value: 'Beta' } }))
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 360)) })
    assert.match(view.container.textContent ?? '', /Beta Plugin/)
    assert.doesNotMatch(view.container.textContent ?? '', /Alpha Skill/)
    const latest = requested.filter(request => request.operation === 'depot.artifacts.list').at(-1)
    assert.equal(latest?.input.query, 'Beta')
  } finally { await view.unmount(); restore() }
})

test('a stale detail response cannot overwrite a newer artifact selection', async () => {
  const originalFetch = globalThis.fetch
  const pending = new Map<string, { resolve: (value: Response) => void; promise: Promise<Response> }>()
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (String(input) === '/v1/depot/operations' && (init?.method ?? 'GET').toUpperCase() === 'GET') return operationCatalog()
    const body = JSON.parse(String(init?.body ?? '{}')) as { operation?: string; params?: Record<string, unknown> }
    if (body.operation === 'depot.artifacts.list') return envelope({ artifacts: [artifact('alpha', 'skill', 'Alpha'), artifact('bravo', 'agent', 'Bravo')], total: 2 })
    if (body.operation === 'depot.artifacts.get') {
      const id = String(body.params?.artifactId)
      let resolve!: (value: Response) => void
      const promise = new Promise<Response>(yes => { resolve = yes })
      pending.set(id, { resolve, promise })
      return promise
    }
    return Response.json({ message: 'unexpected request' }, { status: 500 })
  }) as typeof globalThis.fetch
  document.body.replaceChildren()
  const params = new URLSearchParams({ artifact: 'alpha' })
  const view = await renderClient(<SearchParamsContext.Provider value={params as never}><LibraryPageContent /></SearchParamsContext.Provider>)
  try {
    await flush()
    await view.rerender(<SearchParamsContext.Provider value={new URLSearchParams({ artifact: 'bravo' }) as never}><LibraryPageContent /></SearchParamsContext.Provider>)
    await flush()
    await act(async () => pending.get('bravo')!.resolve(envelope({ artifact: artifact('bravo', 'agent', 'Bravo current') })))
    assert.match(document.body.textContent ?? '', /Bravo current/)
    await act(async () => pending.get('alpha')!.resolve(envelope({ artifact: artifact('alpha', 'skill', 'Alpha stale') })))
    assert.doesNotMatch(document.body.textContent ?? '', /Alpha stale/)
    assert.match(document.body.textContent ?? '', /Bravo current/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})


test('Library bounds mounted rows and retained export metadata across seven catalog pages', async () => {
  const pages = Array.from({ length: 7 }, (_, page) => Array.from({ length: 200 }, (_, row) => artifact(`artifact-${page}-${row}`, 'skill')))
  const { view, restore } = await renderLibrary(new URLSearchParams(), pages[0], pages)
  const originalCreate = URL.createObjectURL
  const originalRevoke = URL.revokeObjectURL
  let exported: Blob | undefined
  URL.createObjectURL = blob => { exported = blob as Blob; return 'blob:library-test' }
  URL.revokeObjectURL = () => {}
  try {
    await flush()
    for (let page = 1; page < pages.length; page++) {
      const more = [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Load more')
      assert.ok(more)
      await act(async () => more.click())
      await flush()
      const rows = view.container.querySelectorAll('[data-library-collection] button.group, [data-library-collection] tbody tr')
      assert.ok(rows.length <= 200, `page ${page} mounted ${rows.length} artifact rows`)
    }
    assert.match(view.container.textContent ?? '', /Earlier results were discarded/)
    for (let page = 1; page < 5; page++) {
      const next = [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Next page')!
      assert.equal(next.disabled, false)
      await act(async () => next.click())
    }
    assert.match(view.container.textContent ?? '', /artifact-6-199/)
    assert.equal(view.container.querySelectorAll('[data-library-collection] button.group, [data-library-collection] tbody tr').length, 200)
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Export loaded library metadata"]')!.click())
    assert.ok(exported)
    const payload = JSON.parse(await exported.text())
    assert.equal(payload.complete, false, 'an evicted catalog prefix must never be exported as complete')
    assert.equal(payload.total, 1400)
    assert.ok(payload.artifacts.length <= 1000)
    assert.ok(payload.artifacts.some((item: { id: string }) => item.id === 'artifact-6-199'))
    await act(async () => [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Refresh')!.click())
    await flush()
    await act(async () => [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Load more')!.click())
    await flush()
    assert.match(view.container.textContent ?? '', /Page 1 of 2/)
    assert.match(view.container.textContent ?? '', /artifact-0-0/)
    assert.doesNotMatch(view.container.textContent ?? '', /Earlier results were discarded/)
  } finally {
    URL.createObjectURL = originalCreate
    URL.revokeObjectURL = originalRevoke
    await view.unmount()
    restore()
  }
})


test('Library explains capacity eviction without claiming that a populated catalog is empty', async () => {
  const oversized = artifact('huge', 'skill')
  oversized.description = 'x'.repeat(9 * 1024 * 1024)
  const { view, restore } = await renderLibrary(new URLSearchParams(), [oversized])
  try {
    await flush()
    assert.match(view.container.textContent ?? '', /No results fit in the retained window/)
    assert.doesNotMatch(view.container.textContent ?? '', /No artifacts in your library yet/)
    assert.equal(view.container.querySelector<HTMLButtonElement>('button[aria-label="Export loaded library metadata"]')!.disabled, true)
  } finally { await view.unmount(); restore() }
})

test('Library unavailable counts stay unknown and retry recovers actual records', async () => {
  const originalFetch = globalThis.fetch
  let unavailable = true
  let lists = 0
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if ((init?.method ?? 'GET') === 'GET' && String(input) === '/v1/depot/operations') return unavailable ? new Response('<html>bad gateway</html>', { status: 502 }) : operationCatalog()
    const body = JSON.parse(String(init?.body ?? '{}'))
    if (body.operation === 'depot.artifacts.list') { lists++; return envelope({ artifacts: [artifact('actual', 'skill', 'Actual Library record')], total: 1 }) }
    return Response.json({}, { status: 500 })
  }) as typeof fetch
  document.body.replaceChildren()
  const view = await renderClient(<SearchParamsContext.Provider value={new URLSearchParams() as never}><LibraryPageContent /></SearchParamsContext.Provider>)
  try {
    await flush()
    assert.match(view.container.textContent ?? '', /Count unavailable/)
    assert.doesNotMatch(view.container.textContent ?? '', /0 of 0|No artifacts in your library yet/)
    assert.equal((view.container.querySelector('[data-console-hero-stats]')?.textContent?.match(/—/g) ?? []).length, 5)
    unavailable = false
    const retry = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Retry loading'))!
    await act(async () => retry.click())
    await flush()
    assert.match(view.container.textContent ?? '', /Actual Library record/)
    assert.match(view.container.textContent ?? '', /1 of 1/)
    assert.doesNotMatch(view.container.textContent ?? '', /Library unavailable|Count unavailable/)
    const refresh = [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Refresh')!
    await act(async () => { refresh.click(); refresh.click() })
    await flush()
    assert.equal(lists, 2, 'repeated same-turn refresh activations share one catalog request')
    assert.match(view.container.textContent ?? '', /Actual Library record/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('Library empty success differs from failed catalogs and partial facets remain lower bounds', async () => {
  const empty = await renderLibrary(new URLSearchParams(), [])
  try { await flush(); assert.match(empty.view.container.textContent ?? '', /No artifacts in your library yet/); assert.match(empty.view.container.textContent ?? '', /0 of 0/) }
  finally { await empty.view.unmount(); empty.restore() }
  const partial = await renderLibrary(new URLSearchParams(), [artifact('one', 'skill')], [[artifact('one', 'skill')], [artifact('two', 'skill')]])
  try {
    await flush()
    assert.match(partial.view.container.textContent ?? '', /1 of 2/)
    assert.match(partial.view.container.textContent ?? '', /Facet counts describe loaded artifacts only/)
    assert.match(partial.view.container.querySelector('[data-console-hero-stats]')?.textContent ?? '', /Public1\+/)
  } finally { await partial.view.unmount(); partial.restore() }
})


test('Library does not invent a global total when a paginated authority omits it', async () => {
  const fixture = await renderLibrary(new URLSearchParams(), [artifact('one', 'skill')], [[artifact('one', 'skill')], [artifact('two', 'skill')]], true)
  try {
    await flush()
    assert.match(fixture.view.container.textContent ?? '', /1 of 1\+/)
    const loadMore = [...fixture.view.container.querySelectorAll('button')].find(button => button.textContent === 'Load more')!
    await act(async () => loadMore.click())
    await flush()
    assert.match(fixture.view.container.textContent ?? '', /2 of 2/)
    assert.doesNotMatch(fixture.view.container.textContent ?? '', /Facet counts describe loaded artifacts only/)
  } finally { await fixture.view.unmount(); fixture.restore() }
})
