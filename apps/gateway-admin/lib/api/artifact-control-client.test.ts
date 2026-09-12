import assert from 'node:assert/strict'
import test from 'node:test'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { controlPlaneAction, uploadArtifactBytes } from './artifact-control-client.ts'

test('control-plane actions use only the selected Labby service and action', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: true, projectId: 'project' })
  const originalFetch = globalThis.fetch
  let capturedUrl = ''
  let capturedBody = ''
  globalThis.fetch = async (input, init) => {
    capturedUrl = String(input)
    capturedBody = String(init?.body)
    return new Response(JSON.stringify({ sources: [] }), { status: 200, headers: { 'content-type': 'application/json' } })
  }
  try {
    await controlPlaneAction('sources', 'sources.list', {})
    assert.equal(capturedUrl, '/v1/sources')
    assert.deepEqual(JSON.parse(capturedBody), { action: 'sources.list', params: {} })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('raw upload stays project-bound and never serializes bytes into action JSON', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: true, projectId: 'project-1' })
  const originalFetch = globalThis.fetch
  let captured: { url: string; init?: RequestInit } | undefined
  globalThis.fetch = async (input, init) => {
    captured = { url: String(input), init }
    return new Response(JSON.stringify({ upload: { id: 'up-1', status: 'ready' } }), { status: 200, headers: { 'content-type': 'application/json' } })
  }
  try {
    const file = new File(['opaque bytes'], 'skills.zip', { type: 'application/zip' })
    await uploadArtifactBytes('up/1', file, 'primary')
    assert.equal(captured?.url, '/v1/uploads/up%2F1?connection_id=primary')
    assert.equal(captured?.init?.method, 'PUT')
    assert.equal(captured?.init?.body, file)
    const headers = new Headers(captured?.init?.headers)
    assert.equal(headers.get('x-csrf-token'), 'csrf')
    assert.equal(headers.get('x-labby-project-id'), 'project-1')
    assert.equal(headers.get('authorization'), null)
  } finally {
    globalThis.fetch = originalFetch
  }
})


test('raw upload refuses a retry or success after its project changes', async () => {
  const originalFetch = globalThis.fetch
  const session = { status: 'authenticated' as const, user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: true, projectId: 'project-1' }
  try {
    for (const status of [200, 403]) {
      __setBrowserSessionStateForTests(session)
      let calls = 0
      globalThis.fetch = async () => {
        calls++
        __setBrowserSessionStateForTests({ ...session, projectId: 'project-2', csrfToken: 'new-csrf' })
        return new Response(JSON.stringify(status === 200 ? { ok: true } : { kind: 'auth_failed' }), { status })
      }
      await assert.rejects(uploadArtifactBytes('upload-1', new File(['bytes'], 'test.zip')), { name: 'AbortError' })
      assert.equal(calls, 1)
    }
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('raw upload stays cancelled after returning to the same principal and project', async () => {
  const originalFetch = globalThis.fetch
  const session = { status: 'authenticated' as const, user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', projectId: 'project-1' }
  try {
    for (const interim of [{ ...session, projectId: 'project-2' }, { status: 'unauthenticated' as const }]) {
      for (const status of [200, 403]) {
        __setBrowserSessionStateForTests(session)
        let calls = 0
        globalThis.fetch = async () => {
          calls++
          __setBrowserSessionStateForTests(interim)
          __setBrowserSessionStateForTests({ ...session, csrfToken: 'new-csrf' })
          return new Response(JSON.stringify(status === 200 ? { ok: true } : { kind: 'auth_failed' }), { status })
        }
        await assert.rejects(uploadArtifactBytes('upload-1', new File(['bytes'], 'test.zip')), { name: 'AbortError' })
        assert.equal(calls, 1)
      }
    }
  } finally { globalThis.fetch = originalFetch }
})

test('raw upload can retry after a transport-only CSRF refresh', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: 1000, csrfToken: 'old-csrf', projectId: 'project-1' })
  let uploads = 0
  let refreshes = 0
  try {
    globalThis.fetch = async (url, init) => {
      if (String(url) === '/auth/session') {
        refreshes++
        return new Response(JSON.stringify({ authenticated: true, user: { sub: 'operator' }, expires_at: 2000, csrf_token: 'new-csrf', project_id: 'project-1' }))
      }
      uploads++
      if (uploads === 1) return new Response(JSON.stringify({ kind: 'auth_failed' }), { status: 403 })
      assert.equal(new Headers(init?.headers).get('x-csrf-token'), 'new-csrf')
      return new Response(JSON.stringify({ ok: true }))
    }
    assert.deepEqual(await uploadArtifactBytes('upload-1', new File(['bytes'], 'test.zip')), { ok: true })
    assert.equal(uploads, 2)
    assert.equal(refreshes, 1)
  } finally { globalThis.fetch = originalFetch }
})
