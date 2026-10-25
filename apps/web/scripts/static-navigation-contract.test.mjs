import test from 'node:test'
import assert from 'node:assert/strict'
import { verifyStaticNavigationBuildId } from './static-navigation-contract.mjs'

const buildId = 'candidate-123'
const html = '<html><script src="/_next/static/chunks/example.js"></script></html>'
// Next 16.4's exported __next._tree.txt has numbered RSC records and a root b field.
const root = value => `3:[]\n2:[["children",{"s":"loadouts","h":96,"c":"$Q3"}]]\n0:${JSON.stringify(value)}\n`
const valid = root({ b: buildId, t: { t: { s: '', h: 80, c: '$Q1' } } })
const verify = (tree, content = html) => verifyStaticNavigationBuildId({ buildId, tree, html: content })

test('static navigation accepts the actual Next 16.4 root build field', () => {
  assert.doesNotThrow(() => verify(valid))
})

test('static navigation rejects a stale root even when other fields contain the current build', () => {
  for (const tree of [
    root({ b: 'old', t: { b: buildId } }),
    root({ b: 'old', buildId }),
    `1:${JSON.stringify({ b: buildId })}\n${root({ b: 'old' })}`,
    root({ buildId }),
    `0:${JSON.stringify({ b: 'old' })}\n0:${JSON.stringify({ b: buildId })}\n`,
    `1:${JSON.stringify({ b: buildId })}\n`,
    '0:{malformed}\n',
  ]) assert.throws(() => verify(tree))
})

test('static navigation rejects deployment cache IDs in otherwise valid exports', () => {
  for (const content of [
    '<html data-dpl-id="deployment"></html>',
    '<script src="/_next/chunk.js?dpl=deployment"></script>',
    '<script src="/_next/chunk.js?x=1&dpl=deployment"></script>',
  ]) assert.throws(() => verify(valid, content))
  assert.throws(() => verifyStaticNavigationBuildId({ buildId: '', tree: valid, html }))
})
