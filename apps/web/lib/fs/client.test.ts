import test from 'node:test'
import assert from 'node:assert/strict'

import { __resetPreviewCache, previewWorkspaceFile } from './client.ts'

// Minimal browser-shim. The cache layer constructs URLs against
// window.location.origin, so we provide a stub before the module is exercised.
type GlobalLike = {
  window?: { location: { origin: string } }
  fetch?: typeof fetch
}

const g = globalThis as unknown as GlobalLike
if (!g.window) {
  g.window = { location: { origin: 'http://localhost' } }
}

function makeStreamingResponse(body: string, contentType = 'text/plain'): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode(body))
      controller.close()
    },
  })
  return new Response(stream, {
    status: 200,
    headers: { 'content-type': contentType },
  })
}

test('previewWorkspaceFile dedupes concurrent same-path fetches', async () => {
  __resetPreviewCache()
  let fetchCalls = 0
  g.fetch = (async (_input: RequestInfo | URL) => {
    void _input
    fetchCalls += 1
    return makeStreamingResponse('hello-bytes')
  }) as typeof fetch

  // Three concurrent calls for the same path.
  const [a, b, c] = await Promise.all([
    previewWorkspaceFile('/workspace/img.png'),
    previewWorkspaceFile('/workspace/img.png'),
    previewWorkspaceFile('/workspace/img.png'),
  ])

  assert.equal(fetchCalls, 1, 'fetch should be called exactly once for 3 concurrent same-path requests')
  assert.equal(a.contentType, 'text/plain')
  assert.equal(b.contentType, 'text/plain')
  assert.equal(c.contentType, 'text/plain')
  // All callers see the same bytes (length, not identity — Blob shape varies).
  assert.equal(await a.blob.text(), 'hello-bytes')
  assert.equal(await b.blob.text(), 'hello-bytes')
  assert.equal(await c.blob.text(), 'hello-bytes')
})

test('previewWorkspaceFile differentiates by maxBytes', async () => {
  __resetPreviewCache()
  let fetchCalls = 0
  g.fetch = (async () => {
    fetchCalls += 1
    return makeStreamingResponse('x')
  }) as typeof fetch

  await Promise.all([
    previewWorkspaceFile('/workspace/img.png'),
    previewWorkspaceFile('/workspace/img.png', { maxBytes: 1024 }),
  ])
  assert.equal(fetchCalls, 2, 'different maxBytes should not dedupe')
})

test('previewWorkspaceFile re-fetches after settle', async () => {
  __resetPreviewCache()
  let fetchCalls = 0
  g.fetch = (async () => {
    fetchCalls += 1
    return makeStreamingResponse('y')
  }) as typeof fetch

  await previewWorkspaceFile('/workspace/img.png')
  // Allow the .finally cache cleanup to run.
  await new Promise((resolve) => setTimeout(resolve, 0))
  await previewWorkspaceFile('/workspace/img.png')
  assert.equal(fetchCalls, 2, 'after settle, the next call re-fetches (cache evicts on settle)')
})

test('previewWorkspaceFile evicts on error so retries can succeed', async () => {
  __resetPreviewCache()
  let fetchCalls = 0
  g.fetch = (async () => {
    fetchCalls += 1
    if (fetchCalls === 1) {
      return new Response('{"kind":"internal","message":"boom"}', {
        status: 500,
        headers: { 'content-type': 'application/json' },
      })
    }
    return makeStreamingResponse('ok')
  }) as typeof fetch

  await assert.rejects(() => previewWorkspaceFile('/workspace/img.png'))
  await new Promise((resolve) => setTimeout(resolve, 0))
  const ok = await previewWorkspaceFile('/workspace/img.png')
  assert.equal(await ok.blob.text(), 'ok')
  assert.equal(fetchCalls, 2)
})

test('previewWorkspaceFile post-await abort throws AbortError without cancelling shared fetch', async () => {
  __resetPreviewCache()
  let fetchCalls = 0
  g.fetch = (async () => {
    fetchCalls += 1
    return makeStreamingResponse('z')
  }) as typeof fetch

  const aborter = new AbortController()
  const sharedPromise = previewWorkspaceFile('/workspace/img.png')
  const abortedPromise = previewWorkspaceFile('/workspace/img.png', { signal: aborter.signal })
  aborter.abort()

  await assert.rejects(() => abortedPromise, (err: Error) => err.name === 'AbortError')
  // The shared fetch still resolves for the un-aborted caller.
  const shared = await sharedPromise
  assert.equal(await shared.blob.text(), 'z')
  assert.equal(fetchCalls, 1, 'only one network fetch despite abort')
})
