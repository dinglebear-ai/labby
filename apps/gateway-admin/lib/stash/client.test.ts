import assert from 'node:assert/strict'
import test from 'node:test'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import type { AuthoritySnapshot } from '../auth/authority.ts'
import { STASH_WORKSPACE_UNSUPPORTED, StashError, createGrant, deleteFile, downloadFile, downloadUrl, getStats, listFiles, renameFile, searchRecipients, uploadFile } from './client.ts'
import { resolveStashOwner } from './owner.ts'

const originalFetch = globalThis.fetch
test.beforeEach(() => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stash', isAdmin: false })
})
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

const baseAuthority: AuthoritySnapshot = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'personal', id: 'principal-1' }, teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [{ id: 'project-1', role: 'manager' }], capabilities: ['scope.read'], generation: 1 }
function authenticateWith(authority: AuthoritySnapshot) {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stash', isAdmin: false, authority })
}

test('list preserves opaque cursor and search while using the server page default', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => {
    requested = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ files: [], next_cursor: null })
  }
  await listFiles('opaque cursor', undefined, 'needle')
  assert.equal(new URL(requested?.url || '').pathname, '/v1/stash/')
  assert.equal(new URL(requested?.url || '').searchParams.has('limit'), false)
  assert.equal(new URL(requested?.url || '').searchParams.get('cursor'), 'opaque cursor')
  assert.equal(new URL(requested?.url || '').searchParams.get('query'), 'needle')
  assert.equal(requested?.credentials, 'include')
})

test('binary upload passes the File body and csrf without JSON wrapping', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => {
    requested = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ file_id: '01J', uri: 'stash://me/files/01J' }, { status: 201 })
  }
  const file = new File(['hello'], 'notes & work.md')
  await uploadFile(file)
  const url = new URL(requested?.url || '')
  assert.equal(url.search, '')
  assert.equal(decodeURIComponent(requested?.headers.get('x-labby-stash-filename') || ''), file.name)
  assert.equal(requested?.headers.get('x-csrf-token'), 'csrf-stash')
  assert.equal(requested?.body instanceof ReadableStream, true)
  assert.equal(await requested?.text(), 'hello')
})

test('mutations use encoded identifiers, csrf, and the documented bodies', async () => {
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init); requests.push(request)
    if (request.method === 'DELETE') return new Response(null, { status: 204 })
    if (request.method === 'POST') return Response.json({ grant_id: 'g', file_id: 'a/b', grantee_principal_id: 'p', created_at: 1 }, { status: 201 })
    return Response.json({ file_id: 'a/b', uri: 'stash://me/files/a%2Fb', display_name: 'new', size_bytes: 1, created_at: 1, updated_at: 2, owned: true })
  }
  await renameFile('a/b', 'new')
  await createGrant('a/b', 'p')
  await deleteFile('a/b')
  assert.ok(requests.every(request => request.url.includes('a%2Fb')))
  assert.ok(requests.every(request => request.headers.get('x-csrf-token') === 'csrf-stash'))
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { display_name: 'new' })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { grantee_principal_id: 'p' })
})

test('download URLs are same-origin attachments and encode opaque IDs', () => {
  assert.equal(downloadUrl('a/b'), '/v1/stash/files/a%2Fb/content')
})

test('recipient discovery keeps identity queries out of URLs and requires csrf', async () => {
  let requested: Request | undefined
  globalThis.fetch = async (input, init) => { requested = new Request(new URL(String(input), 'http://labby.test'), init); return Response.json({ recipients: [] }) }
  await searchRecipients('private person')
  assert.equal(new URL(requested?.url || '').search, '')
  assert.equal(requested?.method, 'POST')
  assert.equal(requested?.headers.get('x-csrf-token'), 'csrf-stash')
  assert.deepEqual(JSON.parse(await requested!.text()), { query: 'private person' })
})

test('selected team is sent through owner headers and never through a URL', async () => {
  authenticateWith({ ...baseAuthority, activeOwner: { kind: 'team', id: 'team-1' }, activeTeamId: 'team-1' })
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init); requests.push(request)
    return request.url.endsWith('/content') ? new Response('bytes', { status: 200 }) : Response.json({ files: [], next_cursor: null })
  }
  await listFiles()
  const blob = await downloadFile('file-1')
  assert.equal(await blob.text(), 'bytes')
  assert.equal(requests.length, 2)
  for (const request of requests) {
    assert.equal(request.headers.get('x-labby-owner-kind'), 'team')
    assert.equal(request.headers.get('x-labby-owner-id'), 'team-1')
    assert.equal(new URL(request.url).search, '', 'owner selection must not appear in query strings')
    assert.doesNotMatch(request.url, /team-1|principal-1/, 'owner identifiers must not appear anywhere in the URL')
  }
  assert.equal(new URL(requests[1]!.url).pathname, '/v1/stash/files/file-1/content')
  assert.equal(requests[1]!.credentials, 'include')
  assert.equal(downloadUrl('file-1'), '/v1/stash/files/file-1/content')
})

test('a personal workspace selects the principal stash, and a project workspace selects its bound team', async () => {
  const observed: Array<[string | null, string | null]> = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    observed.push([request.headers.get('x-labby-owner-kind'), request.headers.get('x-labby-owner-id')])
    return Response.json({ owned_file_count: 0, owned_shared_file_count: 0, owned_committed_bytes: 0, owned_reserved_bytes: 0 })
  }
  authenticateWith(baseAuthority)
  await getStats()
  authenticateWith({ ...baseAuthority, activeOwner: { kind: 'project', id: 'project-1' }, activeTeamId: 'team-1', activeProjectId: 'project-1' })
  await getStats()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf-stash' })
  await getStats()
  assert.deepEqual(observed, [['personal', 'principal-1'], ['team', 'team-1'], [null, null]])
})

test('download failures surface the structured stash error', async () => {
  authenticateWith(baseAuthority)
  globalThis.fetch = async () => Response.json({ kind: 'not_found', message: 'no such file' }, { status: 404 })
  await assert.rejects(downloadFile('missing'), (error: unknown) => error instanceof StashError && error.status === 404 && error.kind === 'not_found')
})

test('installation and unbound project workspaces fail explicitly instead of reading the personal stash', async () => {
  let fetched = 0
  globalThis.fetch = async () => { fetched += 1; return Response.json({ files: [], next_cursor: null }) }
  for (const authority of [
    { ...baseAuthority, activeOwner: { kind: 'installation' as const, id: 'installation' } },
    { ...baseAuthority, activeOwner: { kind: 'project' as const, id: 'project-1' }, activeProjectId: 'project-1' },
  ]) {
    authenticateWith(authority)
    for (const call of [() => listFiles(), () => getStats(), () => downloadFile('file-1'), () => uploadFile(new File(['x'], 'x.txt')), () => deleteFile('file-1')]) {
      await assert.rejects(call(), (error: unknown) => error instanceof StashError && error.kind === STASH_WORKSPACE_UNSUPPORTED && error.status === 0 && /Switch to a Personal or Team workspace/.test(error.message))
    }
  }
  assert.equal(fetched, 0, 'no request may be sent for an unsupported workspace')
  assert.deepEqual(resolveStashOwner({ ...baseAuthority, activeOwner: { kind: 'installation', id: 'installation' } }), { ok: false, reason: 'installation' })
  assert.deepEqual(resolveStashOwner({ ...baseAuthority, activeOwner: { kind: 'project', id: 'project-1' } }), { ok: false, reason: 'project_without_team' })
  assert.deepEqual(resolveStashOwner(undefined), { ok: true, owner: undefined })
})
