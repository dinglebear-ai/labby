import test from 'node:test'
import assert from 'node:assert/strict'
import { cadenceSeconds, repositoryInputError } from './depot-managed-sources.tsx'

test('repository refresh schedules accept only representable positive whole intervals', () => {
  assert.equal(cadenceSeconds(1, 'days'), 86400)
  assert.equal(cadenceSeconds(3, 'hours'), 10800)
  for (const value of [0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER]) {
    assert.equal(Number.isNaN(cadenceSeconds(value, 'minutes')), true)
  }
})


test('repository inputs reject malformed URLs, namespace and Git path syntax', () => {
  assert.equal(repositoryInputError('https://github.com/org/repo', 'my-skills', 'main', 'skills/python'), undefined)
  for (const args of [
    ['http://github.com/org/repo', '', '', ''],
    ['https://user:secret@github.com/org/repo', '', '', ''],
    ['https://github.com/org/repo', 'Bad Name', '', ''],
    ['https://github.com/org/repo', '', '-upload-pack', ''],
    ['https://github.com/org/repo', '', 'main..other', ''],
    ['https://github.com/org/repo', '', '', '../outside'],
  ]) assert.ok(repositoryInputError(...args as [string, string, string, string]))
  assert.ok(Number.isNaN(cadenceSeconds(366, 'days')))
})
