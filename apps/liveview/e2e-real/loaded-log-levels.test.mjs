import test from 'node:test'
import assert from 'node:assert/strict'
import {loadedLevelsMatch} from './loaded-log-levels.mjs'

test('a positive loaded-level assertion rejects an empty filtered snapshot', () => {
  assert.equal(loadedLevelsMatch([], 'INFO'), false)
})
test('a positive loaded-level assertion requires every actual row to match', () => {
  assert.equal(loadedLevelsMatch(['INFO', 'INFO'], 'INFO'), true)
  assert.equal(loadedLevelsMatch(['INFO', 'WARN'], 'INFO'), false)
})
