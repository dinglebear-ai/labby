import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'
import { renderToStaticMarkup } from 'react-dom/server'
import { __setBrowserSessionStateForTests, getBrowserSessionContextIdentity } from '@/lib/auth/session-store'

// Radix resolves its layout-effect shim when its modules are first evaluated, so a
// document must exist before the composer (and its portal-based menus) is imported;
// otherwise the actions menu reports open but never mounts its content.
installTestDom()
let ArtifactComposer: typeof import('./artifact-composer').ArtifactComposer
let createArtifactDownload: typeof import('./artifact-composer').createArtifactDownload
let createDraftStorageKey: typeof import('./artifact-composer').createDraftStorageKey
let draftMayContainCredential: typeof import('./artifact-composer').draftMayContainCredential
let renderClient: typeof import('@/lib/testing/dom-test-utils').renderClient
test.before(async () => {
  ;({ renderClient } = await import('@/lib/testing/dom-test-utils'))
  ;({ ArtifactComposer, createArtifactDownload, createDraftStorageKey, draftMayContainCredential } = await import('./artifact-composer'))
})

const authority = {
  schemaVersion: 1 as const, compatibilityGeneration: 1 as const, principalId: 'principal-a', organizationId: 'org',
  activeOwner: { kind: 'project' as const, id: 'project-a' }, activeTeamId: 'team-a', activeProjectId: 'project-a',
  teams: [{ id: 'team-a', role: 'member', membershipEpoch: 1, policyEpoch: 1 }],
  projects: [{ id: 'project-a', role: 'manager' }], capabilities: ['scope.read', 'scope.create'], generation: 1,
}

function authenticate(sub = 'operator-a', project = 'project-a') {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', projectId: project, authority: { ...authority, principalId: sub, activeOwner: { kind: 'project', id: project }, activeProjectId: project, projects: [{ id: project, role: 'manager' }] } })
}

const waitForAutosave = () => new Promise(resolve => setTimeout(resolve, 180))
function setControlValue(window: ReturnType<typeof installTestDom>, control: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const prototype = control.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype
  Object.getOwnPropertyDescriptor(prototype, 'value')?.set?.call(control, value)
  control.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
}

test('artifact title is editable inline without enabling publishing', () => {
  const html = renderToStaticMarkup(<ArtifactComposer />)
  const name = html.match(/<input[^>]*aria-label="Artifact name"[^>]*>/)?.[0]
  assert.ok(name)
  assert.match(name, /placeholder="untitled-artifact"/)
  assert.match(name, /spellCheck="false"/i)
  assert.match(name, /text-\[28px\]/)
  assert.match(name, /border-bottom-style:dotted/)
  assert.match(name, /focus:border-aurora-accent-primary/)
  assert.match(name, /value="repo-triage"/)
  assert.match(html, /<button[^>]*aria-label="Publish skill"[^>]*disabled=""[^>]*bg-aurora-accent-pink[^>]*><svg[^>]*lucide-upload[^>]*>.*<\/svg>Publish<\/button>/)
  assert.match(html, /aria-label="Change artifact kind: Skill" data-visible-label="1"/)
  assert.doesNotMatch(html, /mx-auto mt-4 max-w-5xl/)
  assert.match(html, /grid items-start gap-3 lg:grid-cols/)
  assert.match(html, /data-console-hero-variant="authoring"/)
  assert.match(html, /Depot · Authoring/)
  assert.doesNotMatch(html, /Creation toolbar/)
  assert.doesNotMatch(html, /compiles to every install format/)
  assert.ok(html.indexOf('Depot Operations') < html.indexOf('aria-label="More artifact actions"'))
  assert.ok(html.indexOf('aria-label="More artifact actions"') < html.indexOf('aria-label="Publish skill"'))
  assert.doesNotMatch(html, /aria-label="Artifact type:/)
  assert.match(html, /aurora-accent-pink-deep/)
  const description = html.match(/<textarea[^>]*aria-label="Artifact description"[^>]*>/)?.[0]
  assert.ok(description)
  assert.match(description, /rows="1"/)
  assert.match(description, /border-bottom-style:dotted/)
  assert.match(description, /focus:border-aurora-accent-primary/)
  assert.match(html, /aria-label="Insert a section"/)
  assert.ok(html.indexOf('aria-label="Italic"') < html.indexOf('aria-label="Inline code"'))
  assert.ok(html.indexOf('aria-label="Inline code"') < html.indexOf('aria-label="Bulleted list"'))
  assert.doesNotMatch(html, /Checking publishing access|Publishing is unavailable for this session/)
  assert.doesNotMatch(html, />\/ When to use<|>\/ Steps<|>\/ Examples<|>\/ Constraints</)
  assert.match(html, /aria-label="Artifact document status"/)
  assert.match(html, /skills\/repo-triage\/SKILL.md/)
  assert.match(html, /min-h-\[360px\]/)
  assert.match(html, /text-\[12.5px\] leading-\[1.7\]/)
  const kindStat = html.indexOf('data-console-hero-stat="Kind"')
  const validationStat = html.indexOf('data-console-hero-stat="Validation"')
  const formatStat = html.indexOf('data-console-hero-stat="Formats"')
  assert.ok(kindStat < validationStat && validationStat < formatStat)
  assert.doesNotMatch(html, /data-console-hero-stat="Workspace"/)
  assert.match(html, /Markdown source/)
})

test('writing tips toggle reclaims editor width without resetting the draft', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  // Radix measures the menu trigger with ResizeObserver, which happy-dom does not expose globally.
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, value: window.ResizeObserver ?? class { observe() {} unobserve() {} disconnect() {} } })
  const priorFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(JSON.stringify({ available: false, reason: 'fixture_read_only' }))
  const view = await renderClient(<ArtifactComposer />)
  try {
    // The writing-tips toggle lives in the "More artifact actions" menu. Radix opens
    // the menu on pointerdown, so a plain click on the trigger is not enough.
    const toggleTips = async () => {
      const trigger = view.container.querySelector<HTMLButtonElement>('[aria-label="More artifact actions"]')!
      assert.ok(trigger)
      await act(async () => {
        trigger.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0 }) as unknown as Event)
        trigger.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true, button: 0 }) as unknown as Event)
        trigger.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event)
      })
      const item = Array.from(document.querySelectorAll<HTMLElement>('[role="menuitem"]')).find(node => /writing tips/.test(node.textContent ?? ''))!
      assert.ok(item, 'the writing tips toggle must be listed in the actions menu')
      await act(async () => item.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event))
    }
    const tips = view.container.querySelector<HTMLElement>('#artifact-writing-tips')!
    const editor = view.container.querySelector<HTMLTextAreaElement>('[aria-label="Artifact content"]')!
    const chrome = view.container.querySelector('[aria-label="Document controls"]')!
    assert.ok(chrome.querySelector('[aria-label="Change artifact kind: Skill"]'))
    assert.ok(chrome.querySelector('[aria-label="Document view"]'))
    assert.match(chrome.textContent!, /authenticated workspace required/)
    assert.equal(chrome.parentElement, editor.closest('section'))
    const draft = editor.value
    const gutter = editor.parentElement!.querySelector<HTMLElement>('[aria-hidden="true"] > div')!
    assert.equal(gutter.textContent!.split('\n').length, draft.split('\n').length)
    assert.equal(editor.getAttribute('wrap'), 'off')
    editor.scrollTop = 42
    await act(async () => editor.dispatchEvent((new window.Event('scroll', { bubbles: true }) as unknown as Event)))
    assert.equal(gutter.style.transform, 'translateY(-42px)')
    assert.doesNotMatch(editor.closest('section')!.className, /min-h-\[680px\]/)
    assert.equal(tips.hidden, false)
    await toggleTips()
    assert.equal(tips.hidden, true)
    assert.doesNotMatch(tips.parentElement!.className, /lg:grid-cols/)
    assert.equal(editor.value, draft)
    await toggleTips()
    assert.equal(tips.hidden, false)
    assert.equal(editor.value, draft)
    const preview = Array.from(view.container.querySelectorAll<HTMLButtonElement>('[aria-label="Document view"] button')).find(button => button.textContent === 'Preview')!
    const source = Array.from(view.container.querySelectorAll<HTMLButtonElement>('[aria-label="Document view"] button')).find(button => button.textContent === 'Source')!
    await act(async () => preview.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(preview.getAttribute('aria-pressed'), 'true')
    assert.equal(editor.closest<HTMLElement>('[data-artifact-source]')!.hidden, true)
    assert.match(view.container.querySelector('[aria-label="Artifact preview"]')!.textContent!, /When to use/)
    assert.ok(view.container.querySelector('[aria-label="Artifact preview"] h2'))
    await act(async () => source.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(editor.closest<HTMLElement>('[data-artifact-source]')!.hidden, false)
    assert.equal(editor.value, draft)
    assert.equal(view.container.querySelector('[aria-label="Artifact preview"]'), null)
    const workspaceButtons = Array.from(view.container.querySelectorAll<HTMLButtonElement>('[aria-label="Creation workspace"] button'))
    assert.equal(workspaceButtons.length, 2)
    assert.ok(workspaceButtons.every(button => button.querySelector('svg')))
    await act(async () => workspaceButtons[1].dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(workspaceButtons[1].getAttribute('aria-pressed'), 'true')
    assert.equal(view.container.querySelector('[aria-label="Artifact content"]'), null)
    assert.match(view.container.querySelector('[aria-label="Bundle draft"]')?.textContent ?? '', /Local bundle draft/)
    assert.match(view.container.querySelector('[aria-label="Draft save status"]')?.textContent ?? '', /authenticated workspace required/)
    assert.doesNotMatch(view.container.querySelector('[aria-label="Bundle draft"]')?.textContent ?? '', /rust-reviewer|pre-commit-guard|changelog-writer/)
    assert.equal(view.container.querySelector<HTMLTextAreaElement>('[aria-label="Skill bundle references"]')?.value, '')
    assert.equal(view.container.querySelector<HTMLButtonElement>('[aria-label="Publish skill"]')?.disabled, true)
    await act(async () => workspaceButtons[0].dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(workspaceButtons[0].getAttribute('aria-pressed'), 'true')
    assert.equal(view.container.querySelector<HTMLTextAreaElement>('[aria-label="Artifact content"]')!.value, draft)
  } finally { await view.unmount(); globalThis.fetch = priorFetch }
})

test('draft storage is authority scoped and refuses credential values', () => {
  assert.notEqual(createDraftStorageKey('authenticated:operator-a:project-a'), createDraftStorageKey('authenticated:operator-b:project-a'))
  assert.equal(draftMayContainCredential('TOKEN=${DEPOT_TOKEN}'), false)
  assert.equal(draftMayContainCredential('{"env":{"API_TOKEN":"real-secret-value"}}'), true)
  assert.equal(draftMayContainCredential('Authorization: Bearer actual-token-value'), true)
})

test('download data contains the complete authored source with a usable filename', () => {
  assert.deepEqual(createArtifactDownload('artifact', 'Skill', 'repo-triage', '---\nname: repo-triage\n---\n\n# Body', {}), {
    filename: 'repo-triage-SKILL.md',
    type: 'text/markdown;charset=utf-8',
    content: '---\nname: repo-triage\n---\n\n# Body',
  })
  assert.deepEqual(createArtifactDownload('bundle', 'Skill', 'ops-kit', '', { Skill: 'team/repo-triage\nteam/reviewer' }), {
    filename: 'ops-kit-bundle.json',
    type: 'application/json;charset=utf-8',
    content: '{\n  "name": "ops-kit",\n  "artifacts": {\n    "Skill": [\n      "team/repo-triage",\n      "team/reviewer"\n    ]\n  }\n}',
  })
})

test('clean drafts restore only inside the same authority workspace and secret drafts are removed', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, value: window.ResizeObserver ?? class { observe() {} unobserve() {} disconnect() {} } })
  const priorFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(JSON.stringify({ available: false, reason: 'fixture_read_only' }))
  window.localStorage.clear()
  authenticate()
  const key = createDraftStorageKey(getBrowserSessionContextIdentity())
  let view = await renderClient(<ArtifactComposer />)
  try {
    const name = view.container.querySelector<HTMLInputElement>('[aria-label="Artifact name"]')!
    await act(async () => { setControlValue(window, name, 'workspace-a-draft'); await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => { await waitForAutosave() })
    assert.match(view.container.querySelector('[aria-label="Draft save status"]')?.textContent ?? '', /Saved locally/)
    assert.ok(window.localStorage.getItem(key)?.includes('workspace-a-draft'))
    await view.unmount()

    view = await renderClient(<ArtifactComposer />)
    assert.equal(view.container.querySelector<HTMLInputElement>('[aria-label="Artifact name"]')?.value, 'workspace-a-draft')
    await view.unmount()

    authenticate('operator-b', 'project-b')
    view = await renderClient(<ArtifactComposer />)
    assert.equal(view.container.querySelector<HTMLInputElement>('[aria-label="Artifact name"]')?.value, 'repo-triage')
    await view.unmount()

    authenticate()
    view = await renderClient(<ArtifactComposer />)
    const content = view.container.querySelector<HTMLTextAreaElement>('[aria-label="Artifact content"]')!
    await act(async () => { setControlValue(window, content, 'TOKEN=real-secret-value'); await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => { await waitForAutosave() })
    assert.equal(window.localStorage.getItem(key), null)
    assert.match(view.container.querySelector('[aria-label="Draft save status"]')?.textContent ?? '', /credential/)
  } finally {
    await view.unmount()
    window.localStorage.clear()
    globalThis.fetch = priorFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('draft controls support tag keyboard editing, editor indentation, and truthful storage failures', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  Object.defineProperty(globalThis, 'ResizeObserver', { configurable: true, value: window.ResizeObserver ?? class { observe() {} unobserve() {} disconnect() {} } })
  const priorFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(JSON.stringify({ available: false, reason: 'fixture_read_only' }))
  window.localStorage.clear()
  authenticate()
  const view = await renderClient(<ArtifactComposer />)
  const storage = window.localStorage
  const originalSetItem = storage.setItem.bind(storage)
  try {
    const tags = view.container.querySelector<HTMLInputElement>('[aria-label="Add a tag"]')!
    await act(async () => { setControlValue(window, tags, 'operations'); await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => tags.dispatchEvent(new window.KeyboardEvent('keydown', { key: ' ', bubbles: true }) as unknown as Event))
    assert.match(tags.parentElement?.textContent ?? '', /#operations/)
    await act(async () => tags.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }) as unknown as Event))
    assert.doesNotMatch(tags.parentElement?.textContent ?? '', /#operations/)

    const content = view.container.querySelector<HTMLTextAreaElement>('[aria-label="Artifact content"]')!
    content.setSelectionRange(0, 0)
    await act(async () => {
      content.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Tab', bubbles: true }) as unknown as Event)
      await new Promise(resolve => requestAnimationFrame(resolve))
    })
    assert.ok(content.value.startsWith('  ## When to use'))
    assert.equal(content.selectionStart, 2)

    Object.defineProperty(storage, 'setItem', { configurable: true, value: () => { throw new DOMException('quota', 'QuotaExceededError') } })
    const name = view.container.querySelector<HTMLInputElement>('[aria-label="Artifact name"]')!
    await act(async () => { setControlValue(window, name, 'quota-failure'); await new Promise(resolve => setTimeout(resolve, 0)) })
    await act(async () => { await waitForAutosave() })
    assert.match(view.container.querySelector('[aria-label="Draft save status"]')?.textContent ?? '', /could not be saved/)
  } finally {
    Object.defineProperty(storage, 'setItem', { configurable: true, value: originalSetItem })
    await view.unmount()
    window.localStorage.clear()
    globalThis.fetch = priorFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
