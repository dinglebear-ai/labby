import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import type { FederatedArtifact } from '../../lib/api/depot-client'

test('inspector renders revision file counts and retains the exact artifact import action', async () => {
  const window = installTestDom()
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  const { DiscoverArtifactInspection } = await import('./discover-artifact-inspection')
  let imported: FederatedArtifact | undefined
  const openChanges: boolean[] = []
  const props = {
    loading: false, open: true, importing: false, onOpenChange: (open: boolean) => { openChanges.push(open) },
    onCopy: () => {}, onExport: () => {},
    onImport: async (artifact: FederatedArtifact) => { imported = artifact },
  }
  const artifact: FederatedArtifact = { providerId: 'catalog', artifactId: 'files', currentRevision: { id: 'exact-revision', fileCount: 0 } }
  const view = await renderClient(<DiscoverArtifactInspection {...props} artifact={artifact} />)
  try {
    const dialog = document.querySelector('[role="dialog"]')!
    assert.ok(dialog.classList.contains('max-w-[720px]'))
    assert.ok(dialog.classList.contains('max-h-[86vh]'))
    assert.ok(document.querySelector('[data-slot="dialog-title"]')?.classList.contains('text-[18px]'))
    assert.equal(document.querySelector('[aria-label="Verified publisher"]'), null)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, publisherVerified: true }}/>)
    assert.ok(document.querySelector('[aria-label="Verified publisher"]'))
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, metrics: { stars: 12300, installs: 0, forks: -1 } }}/>)
    assert.equal(document.querySelector('[aria-label="12300 stars"]')?.textContent, '12.3K')
    assert.equal(document.querySelector('[aria-label="0 installs"]')?.textContent, '0')
    assert.equal(document.querySelector('[aria-label="-1 forks"]'), null)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={artifact}/>)
    assert.equal(document.querySelector('[aria-label="12300 stars"]'), null)
    const sourceDetails = document.querySelector('details')!
    assert.equal(sourceDetails.open, false)
    assert.equal(sourceDetails.querySelector('summary')?.textContent, 'Source and revision')
    assert.match(sourceDetails.textContent ?? '', /exact-revision/)
    assert.equal(document.querySelector('button[title="Export metadata"]')?.textContent, 'Export metadata')
    assert.ok(document.querySelector('[data-slot="dialog-footer"]')?.classList.contains('flex-row'))
    assert.ok(!document.querySelector('[data-slot="dialog-footer"]')?.classList.contains('flex-col-reverse'))
    assert.equal(document.querySelector('[aria-label="Artifact tags"]'), null)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, sourceOrigin: 'mcp-registry', provenance: { originalFormat: 'agent-skill', originalVersion: '1' } }} />)
    assert.ok(document.querySelector(`[title="MCP Registry · via ${artifact.providerId}"]`))
    const metadata = [...document.querySelectorAll('dl > div')].map(row => [row.querySelector('dt')?.textContent, row.querySelector('dd')?.textContent])
    assert.ok(metadata.some(([label, value]) => label === 'Source format' && value === 'agent-skill'))
    assert.ok(metadata.some(([label, value]) => label === 'Source format version' && value === '1'))
    assert.doesNotMatch(document.querySelector('[role="dialog"]')?.textContent ?? '', /Supported formats|Verified publisher/)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, readme: { state: 'available', kind: 'readme', path: 'README.md', content: '# Actual document', revisionId: 'exact-revision' } }} />)
    assert.match(document.querySelector('[aria-label="README"]')?.textContent ?? '', /Actual document/)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, readme: { state: 'unavailable', reason: 'not_distributable' } }} />)
    assert.equal(document.querySelector('[aria-label="README"]'), null)
    assert.match(document.querySelector('[aria-label="Document preview"]')?.textContent ?? '', /does not permit a document preview/)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, descriptor: { tags: ['browser', '<script>not markup</script>'] } }} />)
    const tags = document.querySelector('[aria-label="Artifact tags"]')!
    assert.deepEqual([...tags.querySelectorAll('li')].map(tag => tag.textContent), ['browser', '<script>not markup</script>'])
    assert.equal(tags.querySelector('script'), null)
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, descriptor: { tags: [] } }} />)
    assert.equal(document.querySelector('[aria-label="Artifact tags"]'), null)
    for (const count of [0, 1, 2000, undefined]) {
      await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, currentRevision: { ...artifact.currentRevision, fileCount: count } }} />)
      const badges = [...document.querySelectorAll('[role="dialog"] [data-slot="badge"]')].map(node => node.textContent)
      if (count === undefined) assert.ok(!badges.some(label => /^\d+ files?$/.test(label ?? '')))
      else assert.ok(badges.includes(`${count} ${count === 1 ? 'file' : 'files'}`))
    }
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={artifact} />)
    const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Add to Library')
    assert.ok(button)
    await act(async () => { button.click() })
    assert.equal(imported, artifact)
    const closeButton = [...document.querySelectorAll('button')].find(node => node.textContent === 'Close')
    assert.ok(closeButton)
    await act(async () => { closeButton.click() })
    assert.deepEqual(openChanges, [false])
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})
