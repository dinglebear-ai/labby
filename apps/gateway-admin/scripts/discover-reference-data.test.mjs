import test from 'node:test'
import assert from 'node:assert/strict'
import { readReferenceCatalog, referenceFixture } from './discover-reference-data.mjs'

const wrap = table => `<script type="__bundler/template">${JSON.stringify(`this._depot = [${table}];`)}</script>`
const row = (extra = '{ mine: true }') => `A('review', 'Skill', 'jmagar', 'ARD', 'Description', ['review'], '1.2k', '18k', 12, '2d ago', ${extra})`

test('reference parser reads literal catalog data and creates bounded fixture rails', () => {
  const html = wrap(row())
  assert.equal(readReferenceCatalog(html)[0].name, 'review')
  const fixture = referenceFixture(html, Date.parse('2026-09-09T12:00:00Z'))
  assert.equal(fixture.rows.length, 1)
  assert.deepEqual(fixture.memberArtifactIds, ['reference-1'])
  assert.deepEqual(fixture.rows[0].metrics, { stars: 1200, installs: 18000, forks: 12 })
  assert.equal(fixture.rows[0].publisherVerified, false)
  assert.equal(fixture.rows[0].firstSeenAt, '2026-09-07T12:00:00.000Z')
  assert.equal(fixture.rows[0].namespace, 'jmagar')
  assert.equal(fixture.highlights.popular.items[0].installs, 18000)
  assert.equal(fixture.highlights.team.items[0].artifact.providerId, 'team')
  assert.deepEqual(fixture.highlights.loadouts.items, [])
})

test('reference parser rejects executable expressions, getters, spreads and unsafe keys', () => {
  for (const extra of ['runCode()', '{ mine: runCode() }', '{ get mine() { return true } }', '{ ...other }', '{ __proto__: {} }', '{ mine: `computed` }']) {
    assert.throws(() => readReferenceCatalog(wrap(row(extra))))
  }
  assert.throws(() => readReferenceCatalog(wrap('runCode()')))
  assert.throws(() => readReferenceCatalog(wrap(Array(201).fill(row()).join(','))))
  assert.throws(() => readReferenceCatalog('<script>runCode()</script>'))
})

test('reference visibility is preserved only when explicitly supplied', () => {
  for (const visibility of ['Public', 'Team', 'Private']) {
    const fixture = referenceFixture(wrap(row(`{ vis: '${visibility}' }`)))
    assert.equal(fixture.rows[0].publication.visibility, visibility.toLowerCase())
  }
  assert.equal(referenceFixture(wrap(row())).rows[0].publication, undefined)
})
