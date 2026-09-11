import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { installTestDom as installModuleDom } from '@/lib/testing/dom-install'
import { renderToStaticMarkup } from 'react-dom/server'

// Radix resolves its layout-effect shim when its modules are first evaluated, so a
// document must exist before the composer (and its portal-based menus) is imported;
// otherwise the actions menu reports open but never mounts its content.
installModuleDom()
let ArtifactComposer: typeof import('./artifact-composer').ArtifactComposer
test.before(async () => { ({ ArtifactComposer } = await import('./artifact-composer')) })

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
  assert.match(html, /<button[^>]*aria-label="Publish skill"[^>]*disabled=""[^>]*>Publish<\/button>/)
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
    assert.match(chrome.textContent!, /Unsaved draft/)
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
    await act(async () => workspaceButtons[0].dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)))
    assert.equal(workspaceButtons[0].getAttribute('aria-pressed'), 'true')
    assert.equal(view.container.querySelector<HTMLTextAreaElement>('[aria-label="Artifact content"]')!.value, draft)
  } finally { await view.unmount(); globalThis.fetch = priorFetch }
})
