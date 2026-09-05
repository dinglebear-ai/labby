import test from 'node:test'
import assert from 'node:assert/strict'
import { artifactKey, discoveryUrl, parseArtifactKey } from './provider-model.ts'

test('composite identity cannot collide on delimiter-like values', () => {
  assert.notEqual(artifactKey('a:b', 'c'), artifactKey('a', 'b:c'))
  assert.deepEqual(parseArtifactKey(artifactKey('public', 'space + % / 雪')), ['public', 'space + % / 雪'])
})

test('URLSearchParams round trips exact artifact IDs once', () => {
  const raw = 'space + % / 雪'
  const params = new URLSearchParams(discoveryUrl({ provider: 'all', query: 'storage', artifactProvider: 'public', artifact: raw }))
  assert.equal(params.get('artifact'), raw)
  assert.equal(params.get('artifactProvider'), 'public')
})
