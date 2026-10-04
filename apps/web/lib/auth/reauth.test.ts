import test from 'node:test'
import assert from 'node:assert/strict'

import { cancelReauth, pollReauth, startReauth, waitForReauthProof } from './reauth.ts'

const purpose = {
  action: 'provider.save', resource: 'team', version: '7', operation: 'op-1',
  scope: 'lab:admin', payload: { token: 'secret' },
}

test('starts with CSRF and returns only opaque browser interaction state', async () => {
  const calls: Array<[string, RequestInit | undefined]> = []
  const fetcher: typeof fetch = async (url, init) => {
    calls.push([String(url), init])
    return Response.json({ authorizationUrl: 'https://accounts.google.com/auth', interaction: 'opaque', expiresAt: 100 })
  }
  assert.equal((await startReauth(purpose, 'csrf', fetcher)).interaction, 'opaque')
  assert.equal(calls[0]?.[0], '/auth/reauth')
  assert.equal(new Headers(calls[0]?.[1]?.headers).get('x-csrf-token'), 'csrf')
  assert.equal(calls[0]?.[1]?.credentials, 'include')
})

test('poll delivers a proof once and wait aborts on authority epoch change', async () => {
  const completed: typeof fetch = async () => Response.json({ status: 'Completed', proof: 'proof' })
  assert.deepEqual(await pollReauth('opaque', completed), { status: 'Completed', proof: 'proof' })
  await assert.rejects(
    waitForReauthProof('opaque', 4, { fetcher: async () => Response.json({ status: 'Pending' }), epoch: () => 5, delay: async () => {} }),
    /session changed/,
  )
})

test('cancel is session credentialed and CSRF protected', async () => {
  let request: RequestInit | undefined
  await cancelReauth('opaque', 'csrf', async (_url, init) => {
    request = init
    return new Response(null, { status: 204 })
  })
  assert.equal(request?.method, 'DELETE')
  assert.equal(new Headers(request?.headers).get('x-csrf-token'), 'csrf')
})

for (const change of ['epoch', 'abort'] as const) {
  test(`completed proof is rejected when ${change} changes while the poll is pending`, async () => {
    let epoch = 4
    const controller = new AbortController()
    let complete!: (response: Response) => void
    const pending = waitForReauthProof('opaque', 4, {
      epoch: () => epoch,
      signal: controller.signal,
      fetcher: async (_url, init) => {
        assert.equal(init?.signal, controller.signal)
        return new Promise<Response>(resolve => { complete = resolve })
      },
    })
    if (change === 'epoch') epoch = 5
    else controller.abort()
    complete(Response.json({ status: 'Completed', proof: 'stale-proof' }))
    await assert.rejects(pending, change === 'epoch' ? /session changed/ : /abort/i)
  })
}

test('proof polling stops promptly during its default delay when cancelled', async () => {
  const controller = new AbortController()
  let calls = 0
  const pending = waitForReauthProof('opaque', 4, {
    epoch: () => 4, signal: controller.signal,
    fetcher: async () => { calls += 1; return Response.json({ status: 'Pending' }) },
  })
  await new Promise(resolve => setTimeout(resolve, 0))
  controller.abort()
  await assert.rejects(pending, /abort/i)
  assert.equal(calls, 1)
})
