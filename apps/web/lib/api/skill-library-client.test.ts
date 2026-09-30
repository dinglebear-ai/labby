import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { skillLibrary } from './skill-library-client.ts'

test('skill library client sends project-bound create semantics', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })
  let requestBody: unknown
  let requestUrl = ''
  let requestHeaders: Record<string, string> = {}
  globalThis.fetch = (async (input, init) => {
    requestUrl = String(input)
    requestBody = JSON.parse(String(init?.body))
    requestHeaders = init?.headers as Record<string, string>
    return new Response(JSON.stringify({
      artifact_id: 'skill-1',
      committed_library_version: 2,
      published_library_version: 1,
      new_generation: 1,
      relist_required: true,
      relist_guidance: 'List again.',
    }), { status: 200 })
  }) as typeof fetch

  await skillLibrary.create({
    name: 'fleet-health',
    files: [{ path: 'SKILL.md', content: '# Fleet health' }],
    visibility: 'private',
    expectedLibraryVersion: 1,
    idempotencyKey: 'request-1',
  })

  assert.equal(requestHeaders['x-labby-project-id'], 'project-42')
  assert.equal(requestUrl, '/v1/artifacts')
  assert.deepEqual(requestBody, {
    action: 'artifacts.create',
    params: {
      name: 'fleet-health',
      files: [{ path: 'SKILL.md', content: '# Fleet health' }],
      visibility: 'private',
      expected_library_version: 1,
      idempotency_key: 'request-1',
    },
  })
})

test('artifact library search uses the artifact search contract', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      library_version: 1,
      published_library_version: 1,
      can_create: true,
      create_visibilities: ['private'],
      allowed_actions: [],
      items: [],
    }), { status: 200 })
  }) as typeof fetch

  await skillLibrary.list('fleet health')

  assert.deepEqual(requestBody, {
    action: 'artifacts.search',
    params: { query: 'fleet health', limit: 100 },
  })
})

test('artifact library pagination forwards the opaque cursor', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'browser-user' }, expiresAt: 42,
    csrfToken: 'csrf-123', projectId: 'project-42',
  })
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      library_version: 1, published_library_version: 1, can_create: true,
      create_visibilities: ['private'], allowed_actions: [], items: [],
    }), { status: 200 })
  }) as typeof fetch

  await skillLibrary.list('', undefined, 'next-100')

  assert.deepEqual(requestBody, {
    action: 'artifacts.list',
    params: { cursor: 'next-100', limit: 100 },
  })
})

test('artifact revision and archive methods retain exact concurrency guards', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })
  const requests: Array<{ action: string; params: object }> = []
  globalThis.fetch = (async (_input, init) => {
    requests.push(JSON.parse(String(init?.body)))
    return new Response(JSON.stringify({
      artifact_id: 'skill-1', revision_id: 'revision-2', path: 'SKILL.md', text: '# Revised',
      committed_library_version: 3, published_library_version: 2,
      new_generation: 2, relist_required: true, relist_guidance: 'List again.',
    }), { status: 200 })
  }) as typeof fetch

  await skillLibrary.read('skill-1', 'revision-1', 'SKILL.md')
  await skillLibrary.save({
    artifactId: 'skill-1', revisionId: 'revision-1',
    files: [{ path: 'SKILL.md', content: '# Revised' }],
    expectedLibraryVersion: 2, idempotencyKey: 'save-1',
  })
  await skillLibrary.archive({
    artifactId: 'skill-1', expectedLibraryVersion: 3, idempotencyKey: 'archive-1',
  })

  assert.deepEqual(requests, [
    { action: 'artifacts.read', params: { artifact_id: 'skill-1', revision_id: 'revision-1', path: 'SKILL.md' } },
    { action: 'artifacts.save', params: { artifact_id: 'skill-1', expected_revision_id: 'revision-1', files: [{ path: 'SKILL.md', content: '# Revised' }], expected_library_version: 2, idempotency_key: 'save-1' } },
    { action: 'artifacts.archive', params: { artifact_id: 'skill-1', expected_library_version: 3, idempotency_key: 'archive-1' } },
  ])
})

test('artifact import sends only an exact configured source selector', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 42,
    csrfToken: 'csrf-123',
    projectId: 'project-42',
  })
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      artifact_id: 'skill-1', committed_library_version: 2,
      published_library_version: 1, new_generation: 1,
      relist_required: true, relist_guidance: 'List again.',
    }), { status: 200 })
  }) as typeof fetch

  await skillLibrary.import({
    source: {
      kind: 'repository', connection_id: 'team-skills', artifact_id: 'skill-1',
      revision_id: `sha256:${'a'.repeat(64)}`,
    },
    expectedLibraryVersion: 1,
    idempotencyKey: 'import-1',
  })

  assert.deepEqual(requestBody, {
    action: 'artifacts.import',
    params: {
      source: {
        kind: 'repository', connection_id: 'team-skills', artifact_id: 'skill-1',
        revision_id: `sha256:${'a'.repeat(64)}`,
      },
      expected_library_version: 1,
      idempotency_key: 'import-1',
    },
  })
})
