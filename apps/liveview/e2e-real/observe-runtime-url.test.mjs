import test from 'node:test'
import assert from 'node:assert/strict'

test('query alias preserves dot-only names under the standard browser URL parser', () => {
  for (const name of ['.', '..']) {
    const target = new URL(`/gateway/upstream-runtime?${new URLSearchParams({name})}`, 'https://console.example')
    assert.equal(target.pathname, '/gateway/upstream-runtime')
    assert.equal(target.searchParams.get('name'), name)
    const oldTarget = new URL(`/gateway/upstreams/${name}/runtime`, 'https://console.example')
    assert.notEqual(oldTarget.pathname, `/gateway/upstreams/${name}/runtime`)
  }
})
