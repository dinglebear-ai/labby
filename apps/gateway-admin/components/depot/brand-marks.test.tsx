import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { AnthropicMark, LocalBrandMark, LOCAL_BRAND_SLUGS } from './brand-marks'

test('Anthropic mark uses the vendored SVG locally instead of a text placeholder', () => {
  const html = renderToStaticMarkup(<AnthropicMark />)
  assert.match(html, /aria-label="Anthropic"/)
  assert.match(html, /viewBox="0 0 24 24"/)
  assert.match(html, /M17\.3041 3\.541/)
  assert.doesNotMatch(html, /<img|https:/)
  const picker = renderToStaticMarkup(<LocalBrandMark name="Claude Code" />)
  assert.match(picker, /<svg/)
  assert.doesNotMatch(picker, />AI</)
})

test('vendored distro, toolchain, and agent brands render local SVG paths', () => {
  assert.equal(LOCAL_BRAND_SLUGS.Tailscale, 'tailscale')
  for (const [name, slug] of Object.entries(LOCAL_BRAND_SLUGS)) {
    const html = renderToStaticMarkup(<LocalBrandMark name={name} />)
    assert.ok(html.includes(`data-brand-slug="${slug}"`), name)
    assert.match(html, /<path d="/)
    assert.doesNotMatch(html, /<img|https:/)
  }
})
