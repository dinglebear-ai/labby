import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import {
  DETAIL_CAPABILITIES,
  DetailCapabilityCluster,
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
