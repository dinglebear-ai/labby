import test from 'node:test'
import assert from 'node:assert/strict'

import { authorityResultIsCurrent, deleteRequest, withAuthorityConnection } from './artifact-control-plane.tsx'

test('every destructive control-plane target maps to its canonical action params', () => {
  assert.deepEqual(deleteRequest({ kind: 'source', id: 'source-1' }), {
    service: 'sources', params: { id: 'source-1' },
  })
  assert.deepEqual(deleteRequest({ kind: 'upload', id: 'upload-1' }), {
    service: 'uploads', params: { id: 'upload-1' },
  })
  assert.deepEqual(deleteRequest({ kind: 'bundle', id: 'bundle-1' }), {
    service: 'bundles', params: { slug: 'bundle-1' },
  })
})

test('selected authority is included without mutating caller params', () => {
  const params = { limit: 50 }
  assert.deepEqual(withAuthorityConnection(params, 'depot-east'), {
    limit: 50,
    connection_id: 'depot-east',
  })
  assert.deepEqual(params, { limit: 50 })
  assert.deepEqual(withAuthorityConnection(params, ''), params)
})

test('a delayed authority response is rejected after authority or request generation changes', () => {
  assert.equal(authorityResultIsCurrent('depot-east', 'depot-east', 4, 4), true)
  assert.equal(authorityResultIsCurrent('depot-east', 'depot-west', 4, 4), false)
  assert.equal(authorityResultIsCurrent('depot-east', 'depot-east', 4, 5), false)
})
