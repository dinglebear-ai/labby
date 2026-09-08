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
  const props = {
    loading: false, open: true, importing: false, onOpenChange: () => {},
    onCopy: () => {}, onExport: () => {},
    onImport: async (artifact: FederatedArtifact) => { imported = artifact },
  }
  const artifact: FederatedArtifact = { providerId: 'catalog', artifactId: 'files', currentRevision: { id: 'exact-revision', fileCount: 0 } }
  const view = await renderClient(<DiscoverArtifactInspection {...props} artifact={artifact} />)
  try {
    for (const count of [0, 1, 2000, undefined]) {
      await view.rerender(<DiscoverArtifactInspection {...props} artifact={{ ...artifact, currentRevision: { ...artifact.currentRevision, fileCount: count } }} />)
      const badges = [...document.querySelectorAll('[role="dialog"] [data-slot="badge"]')].map(node => node.textContent)
      if (count === undefined) assert.ok(!badges.some(label => /^\d+ files?$/.test(label ?? '')))
      else assert.ok(badges.includes(`${count} ${count === 1 ? 'file' : 'files'}`))
    }
    await view.rerender(<DiscoverArtifactInspection {...props} artifact={artifact} />)
    const button = [...document.querySelectorAll('button')].find(node => node.textContent === 'Send to Labby')
    assert.ok(button)
    await act(async () => { button.click() })
    assert.equal(imported, artifact)
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})
