import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { discoverMembershipSources, useDiscoverMembership } from './use-discover-membership'

test('membership identity selection is stable across sorting and duplicate inspector entries', () => {
  const first = { providerId: 'catalog', artifactId: 'first', currentRevisionId: 'revision' }
  const second = { providerId: 'team', artifactId: 'first', currentRevisionId: 'revision' }
  const nextRevision = { ...first, currentRevisionId: 'next' }
  const selected = discoverMembershipSources([first, second, nextRevision])
  assert.equal(selected.length, 3)
  assert.deepEqual(discoverMembershipSources([second, first, nextRevision, first]), selected)
  assert.deepEqual(discoverMembershipSources([{ ...first, currentRevisionId: undefined }]), [])
})

test('membership batches visible cards and discards old revision answers', async () => {
  const window = installTestDom()
  const originalFetch = globalThis.fetch
  const batches: number[] = []
  let releaseOld: ((response: Response) => void) | undefined
  const json = (items: unknown[]) => new Response(JSON.stringify({ library_version: 1, items }))
  globalThis.fetch = async (_url, init) => {
    const { params } = JSON.parse(String(init?.body))
    batches.push(params.items.length)
    if (params.items[0].revision_id === 'old') return new Promise(resolve => { releaseOld = resolve })
    return json(params.items.map((item: object) => ({ ...item, status: 'exact_revision_present' })))
  }
  const artifacts = (revision: string) => Array.from({ length: 101 }, (_, index) => ({ providerId: 'catalog', artifactId: `artifact-${index}`, currentRevisionId: revision }))
  function Probe({ revision }: { revision: string }) {
    const items = artifacts(revision)
    const present = useDiscoverMembership(items, 0)
    return <span>{items.filter(present).length} present</span>
  }
  const view = await renderClient(<Probe revision="old" />)
  try {
    assert.ok(releaseOld)
    await view.rerender(<Probe revision="new" />)
    for (let attempt = 0; attempt < 50 && view.container.textContent !== '101 present'; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, '101 present')
    assert.deepEqual(batches, [100, 100, 1])
    await act(async () => { releaseOld!(json([])) })
    assert.equal(view.container.textContent, '101 present')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    await window.happyDOM.close()
  }
})

test('membership never publishes mixed library versions or failed refreshed answers', async () => {
  const window = installTestDom()
  const originalFetch = globalThis.fetch
  const items = Array.from({ length: 101 }, (_, index) => ({ providerId: 'catalog', artifactId: `artifact-${index}`, currentRevisionId: 'revision' }))
  let mode: 'stable' | 'changed' | 'failed' = 'stable'
  let calls = 0
  globalThis.fetch = async (_url, init) => {
    calls++
    const { params } = JSON.parse(String(init?.body))
    if (mode === 'failed') return new Response(JSON.stringify({ message: 'Unavailable' }), { status: 503 })
    return new Response(JSON.stringify({
      library_version: mode === 'changed' && params.items.length === 1 ? 2 : 1,
      items: params.items.map((item: object) => ({ ...item, status: 'exact_revision_present' })),
    }))
  }
  function Probe({ refresh }: { refresh: number }) {
    const present = useDiscoverMembership(items, refresh)
    return <span>{items.filter(present).length} present</span>
  }
  const settle = async (predicate: () => boolean) => {
    for (let attempt = 0; attempt < 50 && !predicate(); attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.ok(predicate())
  }
  const view = await renderClient(<Probe refresh={0} />)
  try {
    await settle(() => view.container.textContent === '101 present')
    mode = 'changed'
    calls = 0
    await view.rerender(<Probe refresh={1} />)
    await settle(() => calls === 2)
    assert.equal(view.container.textContent, '0 present')
    mode = 'stable'
    await view.rerender(<Probe refresh={2} />)
    await settle(() => view.container.textContent === '101 present')
    mode = 'failed'
    calls = 0
    await view.rerender(<Probe refresh={3} />)
    await settle(() => calls === 1)
    assert.equal(view.container.textContent, '0 present')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    await window.happyDOM.close()
  }
})
