import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { GatewayHero } from './gateway-hero'

test('unchecked credential servers prevent a nominal fleet verdict', () => {
  const html = renderToStaticMarkup(<GatewayHero totalServers={1} healthy={0} enabled={1} disconnected={0}
    discoveredTools={0} exposedTools={0} discoveredPrompts={0} exposedPrompts={0}
    discoveredResources={0} exposedResources={0} discoveredSkills={0} exposedSkills={0}
    serverStates={[{ id: 'unchecked', name: 'Unchecked', color: 'var(--aurora-text-muted)', state: 'not checked' }]}
    activeLens="configured" toolsViewActive={false} onLensChange={() => {}} />)
  assert.match(html, /1 not checked or idle/)
  assert.doesNotMatch(html, /all systems nominal/)
})
