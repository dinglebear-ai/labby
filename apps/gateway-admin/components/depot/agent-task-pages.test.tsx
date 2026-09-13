import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'
import { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { AgentRecords, TaskRecords } from './agent-task-pages'
import { ContainerCard } from './dev-containers-page-content'

installTestDom()

const runnableAgent = { agent_id: 'old-agent', owner_kind: 'team', owner_id: 'team-1', version: 1, state: 'active', content_digest: 'sha256:content', repository_digest: 'sha256:repository', image_digest: 'sha256:image', harness_digest: 'sha256:harness', harness_id: 'codex', loadout_digest: 'sha256:loadout', catalog_generation: 'catalog-1' }

test('agent definitions expose their exact configured execution references', () => {
  const html = renderToStaticMarkup(<AgentRecords rows={[{ agent_id: 'agent-one', owner_kind: 'team', owner_id: 'team-one', version: 3, state: 'active', content_digest: 'sha256:content', repository_digest: 'sha256:repository', image_digest: 'sha256:image', harness_digest: 'sha256:harness', harness_id: 'codex-readonly', loadout_digest: 'sha256:loadout', catalog_generation: 'catalog-7' }]} />)
  assert.ok(html.includes('Definitions'))
  assert.ok(html.includes('Owner'))
  assert.ok(html.includes('Revision'))
  assert.ok(html.includes('Harness codex-readonly'))
  assert.doesNotMatch(html, /Claude Code|platform-base|Elapsed/)
})

test('empty Agent workspace offers the first-definition creation path', () => {
  const html = renderToStaticMarkup(<AgentRecords rows={[]} onCreate={() => {}} />)
  assert.match(html, /Create the first definition from an operator-approved runtime bundle/)
  assert.match(html, /Create First Agent/)
})

test('agent record start is enabled only for an exact available harness match', async () => {
  const view = await renderClient(<AgentRecords rows={[runnableAgent]} runnableAgentIds={new Set()} onStart={() => {}} />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-expanded="false"]')!.click())
    assert.equal([...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('New Session'))?.disabled, true)
    await view.rerender(<AgentRecords rows={[runnableAgent]} runnableAgentIds={new Set(['old-agent'])} onStart={() => {}} />)
    assert.equal([...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('New Session'))?.disabled, false)
  } finally { await view.unmount() }
})

test('task row expands its actual result and immutable definition', async () => {
  const view = await renderClient(<TaskRecords rows={[{ task_id: 'task-one', owner_kind: 'personal', owner_id: 'owner-one', agent_id: 'agent-one', agent_version: 4, state: 'failed', attempt: 2, error_code: 'executor_failed' }]} />)
  try {
    const row = document.querySelector<HTMLButtonElement>('[aria-expanded="false"]')!
    await act(async () => row.click())
    assert.equal(row.getAttribute('aria-expanded'), 'true')
    assert.ok(document.body.textContent?.includes('executor_failed'))
    assert.ok(document.body.textContent?.includes('Agent revision'))
    const run = [...document.querySelectorAll('button')].find(button => button.textContent?.includes('Run Now'))!
    assert.equal(run.disabled, true)
    assert.doesNotMatch(document.body.textContent ?? '', /Daily|09:00|operator-console/)
  } finally { await view.unmount() }
})

test('container card preserves real lifecycle callbacks and disables unsupported shell', async () => {
  const operations: string[] = []
  const view = await renderClient(<ContainerCard item={{ instance_id: 'container-one', owner_kind: 'team', owner_id: 'team-one', desired_state: 'running', observed_state: 'running' }} busy={false} canOperate canDelete onOperate={operation => operations.push(operation)} onDestroy={() => operations.push('destroy')} />)
  try {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Stop container-one"]')!.click())
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Reconcile container-one"]')!.click())
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Destroy container-one"]')!.click())
    assert.deepEqual(operations, ['stop', 'reconcile', 'destroy'])
    assert.equal(document.querySelector<HTMLButtonElement>('[aria-label="Shell unavailable for container-one"]')!.disabled, true)
    assert.ok(document.body.textContent?.includes('Not reported'))
  } finally { await view.unmount() }
})

test('read-only container card does not expose lifecycle controls', () => {
  const html = renderToStaticMarkup(<ContainerCard item={{ instance_id: 'read-only', owner_kind: 'team', owner_id: 'team-one', desired_state: 'stopped', observed_state: 'stopped' }} busy={false} canOperate={false} canDelete={false} onOperate={() => {}} onDestroy={() => {}} />)
  assert.doesNotMatch(html, /aria-label="Start read-only"|aria-label="Destroy read-only"/)
})
