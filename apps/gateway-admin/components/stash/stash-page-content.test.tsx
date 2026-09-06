import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils.tsx'
import { StashPageContent } from './stash-page-content.tsx'

installTestDom()
const file = (id: string) => ({ file_id: id, uri: `stash://me/files/${id}`, display_name: `${id}.txt`, size_bytes: 1, created_at: 1, updated_at: 1, owned: true })

test('Stash renders live data and appends the next cursor page', async () => {
  document.body.replaceChildren()
  globalThis.fetch = async input => {
    const url = new URL(String(input), 'http://labby.test')
    if (url.pathname.endsWith('/stats')) return Response.json({ owned_file_count: 2, owned_shared_file_count: 0, owned_committed_bytes: 2, owned_reserved_bytes: 0 })
    return Response.json(url.searchParams.has('cursor') ? { files: [file('second')], next_cursor: null } : { files: [file('first')], next_cursor: 'next' })
  }
  const view = await renderClient(<StashPageContent />)
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })
  assert.match(view.container.textContent || '', /first\.txt/)
  const loadMore = [...view.container.querySelectorAll('button')].find(button => button.textContent?.includes('Load more'))
  assert.ok(loadMore)
  await act(async () => { loadMore.dispatchEvent(new window.MouseEvent('click', { bubbles: true })); await new Promise(resolve => setTimeout(resolve, 10)) })
  assert.match(view.container.textContent || '', /first\.txt.*second\.txt/)
  await view.unmount()
})
