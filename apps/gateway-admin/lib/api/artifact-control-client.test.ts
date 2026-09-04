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
