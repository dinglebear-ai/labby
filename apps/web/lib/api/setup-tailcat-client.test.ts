import assert from 'node:assert/strict'
import test from 'node:test'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { tailcatSetupApi } from './setup-client.ts'

test('typed Tailcat actions bind explicit native project parameters and never send credential bearers', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: 1,
    csrfToken: 'csrf', projectId: 'selected-project' })
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = async (_url, init) => {
    const headers = new Headers(init?.headers)
    assert.equal(headers.get('x-labby-project-id'), null)
    assert.equal(headers.get('authorization'), null)
    assert.equal(headers.get('x-csrf-token'), 'csrf')
    requests.push(JSON.parse(init?.body as string))
    return Response.json({ enabled: true, changed: true, restart_required: true })
  }
  try {
    await tailcatSetupApi.tailcatConfigure({ project_id: 'selected-project', public_resource: 'https://labby.example/sandbox',
      derp_map_url: 'https://tailcat.example/map', node_path: '/fixture/node', dry_run: true })
    await tailcatSetupApi.tailcatEnroll('selected-project', 'operation-key')
    await tailcatSetupApi.tailcatEnable('selected-project', 'credential-id')
    assert.deepEqual(requests.map(item => item.action), ['tailcat.configure', 'tailcat.enroll', 'tailcat.enable'])
    assert.equal(requests[0].params.dry_run, true)
    assert.deepEqual(requests[1].params, { project_id: 'selected-project', idempotency_key: 'operation-key' })
    assert.deepEqual(requests[2].params, { project_id: 'selected-project', credential_id: 'credential-id' })
  } finally {
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
