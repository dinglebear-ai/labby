import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ConsoleHero } from './console-hero'

test('semantic metric icons use the same tint as their values', () => {
  const html = renderToStaticMarkup(<ConsoleHero eyebrow="Observe" title="Usage" stats={[{ label: 'Failed', value: 3, tone: 'var(--aurora-error)', icon: <svg /> }]} />)
  assert.match(html, /data-console-hero-stat-icon="1" style="[^"]*color:var\(--aurora-error\)/)
  assert.match(html, /letter-spacing:-0.01em/)
})

test('hero icon is optional and decorative without replacing the page heading', () => {
  const ordinary = renderToStaticMarkup(<ConsoleHero eyebrow="Control plane" title="Gateway" actions={<button>Action</button>} />)
  assert.doesNotMatch(ordinary, /data-console-hero-icon/)
  assert.doesNotMatch(ordinary, /align-self:flex-start/)
  const discover = renderToStaticMarkup(<ConsoleHero eyebrow="Depot" title="Discover" icon={<svg />} actions={<button>Publish</button>} />)
  assert.match(discover, /data-console-hero-icon="1" aria-hidden="true"/)
  assert.match(discover, /<h1[^>]*>Discover<\/h1>/)
  assert.equal((discover.match(/<h1/g) ?? []).length, 1)
  assert.match(discover, /align-self:flex-start/)
  assert.match(discover, /flex:1 1 20rem/)
  assert.doesNotMatch(ordinary, /flex:1 1 20rem/)
})

test('metric units stay separate from values and are omitted when not supplied', () => {
  const markup = renderToStaticMarkup(<ConsoleHero eyebrow="Depot" title="Discover" stats={[
    { label: 'Indexed', value: 26, suffix: 'artifacts' },
    { label: 'Sources', value: 2, suffix: 'connected backends' },
    { label: 'Last crawl', value: 'Not reported' },
  ]} />)
  assert.equal((markup.match(/data-console-hero-stat-unit=/g) ?? []).length, 2)
  assert.match(markup, />26<span[^>]*>artifacts<\/span>/)
  assert.match(markup, />2<span[^>]*>connected backends<\/span>/)
  assert.match(markup, />Not reported<\/div>/)
})

test('Discover uses the compact reference hero without changing other pages', () => {
  const markup = renderToStaticMarkup(<ConsoleHero variant="discover" eyebrow="Depot" title="Discover" icon={<svg />} description="Reference description" stats={[{ label: 'Indexed', value: 26 }]}><div data-search-slot="true" /></ConsoleHero>)
  assert.match(markup, /data-console-hero-variant="discover"/)
  assert.match(markup, /padding:14px 24px 0/)
  assert.match(markup, /width:44px;height:44px;margin-top:3px/)
  assert.match(markup, /max-width:560px/)
  assert.match(markup, /padding:11px 12px 12px;margin-top:14px/)
  assert.match(markup, /<\/div><div data-search-slot="true"><\/div><\/div>$/)
})
