import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import {
  DETAIL_CAPABILITIES,
  DetailCapabilityCluster,
  DetailExposureCell,
  DetailStatStrip,
  DetailStripCard,
} from './gateway-detail-tabs'

test('DetailCapabilityCluster renders all capabilities as unknown by default', () => {
  const markup = renderToStaticMarkup(<DetailCapabilityCluster />)

  assert.equal(DETAIL_CAPABILITIES.length, 11)
  assert.match(markup, /Capabilities — not reported/)
  assert.match(markup, /Tools — not reported/)
  assert.match(markup, /Progress — not reported/)
  assert.doesNotMatch(markup, /Roots —/)
  assert.equal((markup.match(/background:var\(--gw0-0_30\)/g) ?? []).length, 11)
})

test('DetailCapabilityCluster distinguishes advertised and unavailable capabilities', () => {
  const markup = renderToStaticMarkup(
    <DetailCapabilityCluster
      states={{
        tools: 'supported',
        prompts: 'not_advertised',
      }}
    />,
  )

  assert.match(markup, /1 of 11 capabilities advertised in initialize/)
  assert.match(markup, /Tools — supported/)
  assert.match(markup, /Prompts — not advertised/)
  assert.doesNotMatch(markup, />—<\/span>/)
})

test('server detail exposes stable hooks for its narrow-screen reflow', async () => {
  const markup = renderToStaticMarkup(
    <DetailStatStrip cardCount={2}>
      <DetailExposureCell
        stats={[{ label: 'Tools', icon: null, exposed: 3, discovered: 4 }]}
        onClick={() => {}}
        showEnable={false}
      />
      <DetailStripCard label="Calls" value="12" />
      <DetailStripCard label="Errors" value="2" />
    </DetailStatStrip>,
  )

  assert.match(markup, /data-detail-stat-strip="1"/)
  assert.match(markup, /data-detail-exposure="1"/)
  assert.equal((markup.match(/data-detail-stat-card="1"/g) ?? []).length, 2)

  const css = await readFile(new URL('../../app/globals.css', import.meta.url), 'utf8')
  assert.match(css, /@media \(max-width: 640px\)[\s\S]*\[data-detail-stat-strip\]/)
  assert.match(css, /\[data-detail-stat-strip\][\s\S]*grid-template-columns: repeat\(2, minmax\(0, 1fr\)\)/)
  assert.match(css, /\[data-detail-exposure\][\s\S]*grid-column: 1 \/ -1/)
  assert.match(css, /\[data-detail-heading\][\s\S]*flex-direction: column/)
})
