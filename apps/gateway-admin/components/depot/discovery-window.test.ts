import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { appendDiscoveryPage, createDiscoveryWindow, visibleArtifacts } from './discovery-window'

const artifact = (n: number, description = '') => ({
  providerId: 'public', artifactId: `artifact-${n}`, description,
})

describe('discovery window', () => {
  it('bounds retained pages, rows, projected bytes, and its composite index', () => {
    let window = createDiscoveryWindow()
    for (let page = 0; page < 100; page += 1) {
      window = appendDiscoveryPage(window, Array.from({ length: 50 }, (_, row) => artifact(page * 50 + row)))
    }
    assert.ok(window.pages.length <= 20)
    assert.ok(window.rowCount <= 1000)
    assert.equal(window.index.size, window.rowCount)
    assert.equal(window.evictedRows, 4000)
  })

  it('evicts oversized projected summaries before exceeding eight MiB', () => {
    let window = createDiscoveryWindow()
    const large = 'x'.repeat(900_000)
    for (let page = 0; page < 20; page += 1) window = appendDiscoveryPage(window, [artifact(page, large)])
    assert.ok(window.projectedBytes <= 8 * 1024 * 1024)
    assert.ok(window.rowCount < 20)
    assert.equal(window.historyExpired, true)
  })

  it('mounts at most 200 rows around the requested anchor', () => {
    let window = createDiscoveryWindow()
    for (let page = 0; page < 20; page += 1) {
      window = appendDiscoveryPage(window, Array.from({ length: 50 }, (_, row) => artifact(page * 50 + row)))
    }
    const visible = visibleArtifacts(window, 18)
    assert.equal(visible.items.length, 150)
    assert.equal(visible.leadingRows, 850)
    assert.equal(visible.trailingRows, 0)
  })

  it('deduplicates exact composite identities incrementally', () => {
    let window = appendDiscoveryPage(createDiscoveryWindow(), [artifact(1), artifact(2)])
    window = appendDiscoveryPage(window, [artifact(2), artifact(3)])
    assert.equal(window.rowCount, 3)
    assert.equal(window.index.size, 3)
  })
})
