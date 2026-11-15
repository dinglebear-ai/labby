import {test} from 'node:test'
import assert from 'node:assert/strict'

// Catalog projection must never cause an unqualified private-canary invocation.
for (const [surface, tools] of [
  ['ordinary', [{name: 'echo'}, {name: 'fail'}]],
  ['aggregate', [{name: 'private-http-canary::echo'}, {name: 'foreign::echo'}]],
]) {
  test(`${surface} catalog admits an explicit owned-upstream tool call`, async () => {
    const {privateToolCall} = await import('./private-tool-call.mjs')
    assert.deepEqual(privateToolCall(tools, 'private-http-canary', 'echo', {text: 'private-wire-canary'}), {
      name: 'private-http-canary::echo', arguments: {text: 'private-wire-canary'},
    })
  })
}

test('missing or foreign-only catalog entries never admit the private call', async () => {
  const {privateToolCall} = await import('./private-tool-call.mjs')
  for (const tools of [[], [{name: 'fail'}], [{name: 'foreign::echo'}]]) {
    assert.equal(privateToolCall(tools, 'private-http-canary', 'echo', {}), undefined)
  }
})

test('ordinary stdio catalog keeps the explicit upstream selector and empty arguments', async () => {
  const {privateToolCall} = await import('./private-tool-call.mjs')
  assert.deepEqual(privateToolCall([{name: 'observe_private_config'}], 'private-stdio-canary', 'observe_private_config', {}), {
    name: 'private-stdio-canary::observe_private_config', arguments: {},
  })
})
