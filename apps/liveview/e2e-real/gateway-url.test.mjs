import test from 'node:test'
import assert from 'node:assert/strict'
import {isGatewayURL} from './gateway-url.mjs'

const origin = 'http://127.0.0.1:4298'
test('Gateway landing accepts native query state on the exact console pathname', () => {
  for (const suffix of ['', '?columns=name&direction=asc&layout=table&lens=all&query=&sort=name&source=all']) {
    assert.equal(isGatewayURL(new URL(`${origin}/gateway${suffix}`), origin), true)
  }
})
test('Gateway landing rejects other origins and neighboring paths', () => {
  for (const target of ['http://127.0.0.1:4299/gateway', 'https://127.0.0.1:4298/gateway', 'https://other.example/gateway', `${origin}/gateway-evil`, `${origin}/gateway/`, `${origin}/gateway/servers`, `${origin}/other?next=/gateway`]) {
    assert.equal(isGatewayURL(new URL(target), origin), false, target)
  }
})
