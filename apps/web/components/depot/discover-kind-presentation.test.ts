import test from 'node:test'
import assert from 'node:assert/strict'
import { discoverKindPresentation } from './discover-kind-presentation'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

test('reference kinds retain distinct glyphs instead of unrelated substitutes', () => {
  const paths = {
    acp: 'M3 12h4l3-8 4 16 3-8h4',
    skill: 'M12 2 4 6v6c0 5 3.4 8.6 8 10 4.6-1.4 8-5 8-10V6z',
    command: 'm5 8 4 4-4 4M13 16h6',
    snippet: 'm9 10-2 2 2 2m6-4 2 2-2 2',
    loadout: 'm12 2 9 5-9 5-9-5zM3 12l9 5 9-5M3 17l9 5 9-5',
  }
  for (const [kind, path] of Object.entries(paths)) {
    const markup = renderToStaticMarkup(createElement(discoverKindPresentation(kind).icon))
    assert.ok(markup.includes(`d="${path}"`), `${kind} uses the reference geometry`)
  }
  assert.notEqual(discoverKindPresentation('extension').icon, discoverKindPresentation('plugin').icon)
  assert.notEqual(discoverKindPresentation('hook').icon, discoverKindPresentation('skill').icon)
})

test('cards and inspectors share the Discover family taxonomy', () => {
  for (const [family, kinds] of Object.entries({
    Protocol: ['mcp', 'acp'], Capability: ['skill', 'command', 'snippet'],
    Authored: ['agent', 'prompt'], Bundle: ['plugin', 'extension', 'loadout'], Guard: ['hook'],
  })) {
    for (const kind of kinds) {
      assert.equal(discoverKindPresentation(kind).family, family)
      assert.equal(discoverKindPresentation(kind.toUpperCase()).icon, discoverKindPresentation(kind).icon)
    }
  }
})

test('unknown kinds do not acquire an invented or duplicate family label', () => {
  assert.equal(discoverKindPresentation('artifact').family, null)
  assert.equal(discoverKindPresentation('future-kind').family, null)
})

test('kind identity carries the same themed color and icon tile into cards and inspectors', () => {
  for (const [kind, token] of Object.entries({ mcp: 'aurora-protocol-strong', agent: 'aurora-accent-pink', loadout: 'aurora-success', snippet: 'aurora-accent-strong', plugin: 'aurora-success', command: 'aurora-success', hook: 'aurora-warn' })) {
    const presentation = discoverKindPresentation(kind)
    assert.equal(presentation.color, `var(--${token})`)
    assert.equal(presentation.iconStyle.color, presentation.color)
    assert.match(presentation.iconStyle.backgroundColor, /10%/)
    assert.ok(presentation.iconStyle.borderColor.includes(presentation.tone))
    assert.deepEqual(discoverKindPresentation(kind.toUpperCase()), presentation)
  }
  assert.equal(discoverKindPresentation('unknown').color, 'var(--aurora-text-muted)')
})

test('protocol orange is distinct from warning gold and authored tints use the deeper rose', () => {
  assert.notEqual(discoverKindPresentation('mcp').tone, discoverKindPresentation('hook').tone)
  assert.equal(discoverKindPresentation('mcp').tone, 'var(--aurora-protocol)')
  assert.equal(discoverKindPresentation('agent').tone, 'var(--aurora-accent-pink-deep)')
  assert.equal(discoverKindPresentation('prompt').color, 'var(--aurora-accent-pink-strong)')
})
