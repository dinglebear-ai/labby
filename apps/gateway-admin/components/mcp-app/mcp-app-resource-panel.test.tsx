import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { McpAppResourcePanel, mcpAppFallbackText, selectMcpAppHtml } from './mcp-app-resource-panel.tsx'

test('loads HTML into a sandboxed iframe and keeps it across equivalent rerenders', async () => {
  installTestDom()
  let reads = 0
  const readResource = async () => {
    reads += 1
    return { contents: [{ uri: 'ui://fixture/app.html', mimeType: 'text/html;profile=mcp-app', text: '<main>Fixture app</main>' }] }
  }
  const { container, rerender, unmount } = await renderClient(<McpAppResourcePanel resourceUri="ui://fixture/app.html" readResource={readResource}/>)
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  const iframe = container.querySelector('iframe')
  assert.ok(iframe)
  assert.equal(iframe.getAttribute('sandbox'), 'allow-scripts allow-forms allow-popups allow-downloads')
  assert.doesNotMatch(iframe.getAttribute('sandbox') ?? '', /allow-same-origin/)
  assert.match(iframe.getAttribute('srcdoc') ?? '', /Fixture app/)
  await rerender(<McpAppResourcePanel resourceUri="ui://fixture/app.html" readResource={readResource} appName="Fixture"/>)
  assert.equal(reads, 1)
  await unmount()
})

test('shows a graceful failure while preserving the panel shell', async () => {
  installTestDom()
  const { container, unmount } = await renderClient(<McpAppResourcePanel resourceUri="ui://fixture/app.html" readResource={async () => { throw new Error('secret') }}/>)
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  assert.match(container.textContent ?? '', /Failed to load MCP App resource/)
  assert.doesNotMatch(container.textContent ?? '', /secret/)
  await unmount()
})

test('starts the sandbox document before waiting for its load event', async () => {
  installTestDom()
  const result = { content: [{ type: 'text' as const, text: 'Cloudy' }] }
  const { container, unmount } = await renderClient(<McpAppResourcePanel
    resourceUri="ui://fixture/app.html"
    toolInput={{ city: 'Boston' }}
    toolResult={result}
    readResource={async () => ({ contents: [{ uri: 'ui://fixture/app.html', mimeType: 'text/html;profile=mcp-app', text: '<main>Fixture</main>' }] })}
  />)
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  const iframe = container.querySelector('iframe')
  assert.ok(iframe?.contentWindow)
  assert.match(iframe.getAttribute('srcdoc') ?? '', /Fixture/)
  assert.match(container.querySelector('[data-mcp-app-fallback]')?.textContent ?? '', /Cloudy/)
  await unmount()
})

test('changes URI once, ignores stale reads, and reports iframe failure', async () => {
  installTestDom()
  const resolvers: Array<(value: { contents: Array<{ uri: string; mimeType: string; text: string }> }) => void> = []
  const readResource = () => new Promise<{ contents: Array<{ uri: string; mimeType: string; text: string }> }>((resolve) => resolvers.push(resolve))
  const { container, rerender, unmount } = await renderClient(<McpAppResourcePanel resourceUri="ui://fixture/old.html" readResource={readResource}/>)
  await rerender(<McpAppResourcePanel resourceUri="ui://fixture/new.html" readResource={readResource}/>)
  assert.equal(resolvers.length, 2)
  await act(async () => {
    resolvers[0]({ contents: [{ uri: 'ui://fixture/old.html', mimeType: 'text/html', text: '<main>Old</main>' }] })
    resolvers[1]({ contents: [{ uri: 'ui://fixture/new.html', mimeType: 'text/html', text: '<main>New</main>' }] })
    await Promise.resolve(); await Promise.resolve()
  })
  assert.match(container.querySelector('iframe')?.getAttribute('srcdoc') ?? '', /New/)
  assert.doesNotMatch(container.innerHTML, /Old/)
  await act(async () => {
    container.querySelector('iframe')?.dispatchEvent(new window.Event('error', { bubbles: true }))
    await Promise.resolve(); await Promise.resolve()
  })
  assert.match(container.textContent ?? '', /Failed to load MCP App resource/)
  await unmount()
})

test('does not select an explicitly non-HTML resource for iframe rendering', () => {
  assert.equal(
    selectMcpAppHtml({ contents: [{
      uri: 'ui://fixture/app.html',
      mimeType: 'text/plain',
      text: '<main>must not execute</main>',
    }] }, 'ui://fixture/app.html'),
    null,
  )
})

test('extracts only bounded text content for the visible fallback', () => {
  assert.equal(mcpAppFallbackText({ content: [{ type: 'text', text: 'Cloudy' }, { type: 'image', data: 'secret' }] }), 'Cloudy')
  assert.equal(mcpAppFallbackText({ structuredContent: { secret: true } }), null)
})
