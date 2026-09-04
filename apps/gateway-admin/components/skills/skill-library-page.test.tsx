import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { skillLibrary, type SkillLibraryItem, type SkillLibraryPage } from '../../lib/api/skill-library-client.ts'
import { SkillLibraryPageContent } from './skill-library-page.tsx'

function item(id: string): SkillLibraryItem {
  return {
    artifact_id: id,
    name: id,
    archived: false,
    latest_revision_id: `${id}-revision`,
    visibility: 'private',
    access_label: 'Owner',
    can_mutate: true,
    owner: { relationship: 'owner' },
    provenance: { source: 'local' },
    materialized: true,
    current_generation: 1,
    published_library_version: 1,
    allowed_actions: ['artifacts.save'],
    latest_revision_files: [{ path: 'SKILL.md', digest: 'sha256:test', size: 4 }],
  }
}

async function setInputValue(input: HTMLInputElement, value: string) {
  const propsKey = Object.keys(input).find(key => key.startsWith('__reactProps$'))
  assert.ok(propsKey)
  const props = (input as unknown as Record<string, { onChange?: (event: { target: { value: string } }) => void }>)[propsKey]
  assert.ok(props.onChange)
  await act(async () => {
    props.onChange?.({ target: { value } })
    await Promise.resolve()
  })
}

test('a delayed revision read cannot open against a newly selected Artifact', async () => {
  installTestDom()
  const originalList = skillLibrary.list
  const originalRead = skillLibrary.read
  const page: SkillLibraryPage = {
    library_version: 1,
    published_library_version: 1,
    can_create: true,
    create_visibilities: ['private'],
    allowed_actions: [],
    items: [item('alpha'), item('bravo')],
  }
  let resolveRead!: (value: Awaited<ReturnType<typeof skillLibrary.read>>) => void
  const pendingRead = new Promise<Awaited<ReturnType<typeof skillLibrary.read>>>(resolve => { resolveRead = resolve })
  skillLibrary.list = async () => page
  skillLibrary.read = async () => pendingRead

  const view = await renderClient(<SkillLibraryPageContent />)
  try {
    await act(async () => {})
    const button = (label: string) => [...view.container.querySelectorAll('button')]
      .find(candidate => candidate.textContent?.includes(label)) as HTMLButtonElement
    act(() => button('Edit latest').click())
    act(() => button('bravo').click())
    await act(async () => resolveRead({
      library_version: 1,
      artifact_id: 'alpha',
      revision_id: 'alpha-revision',
      path: 'SKILL.md',
      text: 'alpha contents',
    }))

    assert.equal(view.container.querySelector('textarea'), null)
    assert.match(view.container.textContent ?? '', /bravo/)
    assert.doesNotMatch(view.container.textContent ?? '', /alpha contents/)
    assert.equal(button('Edit latest').disabled, false)
  } finally {
    skillLibrary.list = originalList
    skillLibrary.read = originalRead
    await view.unmount()
  }
})

test('editor fields are locked while delayed validation and save use their snapshot', async () => {
  installTestDom()
  const originalList = skillLibrary.list
  const originalValidate = skillLibrary.validate
  const originalCreate = skillLibrary.create
  const page: SkillLibraryPage = {
    library_version: 1,
    published_library_version: 1,
    can_create: true,
    create_visibilities: ['private'],
    allowed_actions: [],
    items: [],
  }
  let resolveValidation!: (value: Awaited<ReturnType<typeof skillLibrary.validate>>) => void
  const pendingValidation = new Promise<Awaited<ReturnType<typeof skillLibrary.validate>>>(resolve => { resolveValidation = resolve })
  let savedFiles: unknown
  skillLibrary.list = async () => page
  skillLibrary.validate = async () => pendingValidation
  skillLibrary.create = async input => {
    savedFiles = input.files
    return {
      artifact_id: 'created',
      committed_library_version: 2,
      published_library_version: 1,
      new_generation: 1,
      relist_required: true,
      relist_guidance: 'refresh',
    }
  }

  const view = await renderClient(<SkillLibraryPageContent />)
  try {
    await act(async () => {})
    const button = (label: string) => [...view.container.querySelectorAll('button')]
      .find(candidate => candidate.textContent?.includes(label)) as HTMLButtonElement
    await act(async () => button('Create skill').click())
    const textarea = view.container.querySelector('textarea') as HTMLTextAreaElement
    act(() => button('Save immutable revision').click())

    assert.equal(textarea.disabled, true)
    assert.equal((view.container.querySelector('input[aria-label="Skill name"]') as HTMLInputElement).disabled, true)
    assert.equal(button('Create skill').disabled, true)
    assert.equal(button('Import').disabled, true)
    assert.equal(button('Cancel').disabled, true)

    await act(async () => resolveValidation({ valid: true, rejections: [] }))
    assert.deepEqual(savedFiles, [{ path: 'SKILL.md', content: textarea.value }])
  } finally {
    skillLibrary.list = originalList
    skillLibrary.validate = originalValidate
    skillLibrary.create = originalCreate
    await view.unmount()
  }
})

test('a pending import locks cancellation and its source fields until completion', async () => {
  installTestDom()
  const originalList = skillLibrary.list
  const originalImport = skillLibrary.import
  const page: SkillLibraryPage = {
    library_version: 1,
    published_library_version: 1,
    can_create: true,
    create_visibilities: ['private'],
    allowed_actions: [],
    items: [],
  }
  let resolveImport!: (value: Awaited<ReturnType<typeof skillLibrary.import>>) => void
  const pendingImport = new Promise<Awaited<ReturnType<typeof skillLibrary.import>>>(resolve => { resolveImport = resolve })
  skillLibrary.list = async () => page
  skillLibrary.import = async () => pendingImport

  const view = await renderClient(<SkillLibraryPageContent />)
  try {
    await act(async () => {})
    const button = (label: string) => [...view.container.querySelectorAll('button')]
      .find(candidate => candidate.textContent?.includes(label)) as HTMLButtonElement
    await act(async () => button('Import').click())
    const input = (label: string) => view.container.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement
    const form = input('Import connection').closest('form')
    assert.ok(form)
    await setInputValue(input('Import connection'), 'depot')
    await setInputValue(input('Import artifact ID'), 'artifact-1')
    await setInputValue(input('Import revision ID'), 'revision-1')
    const submit = view.container.querySelector('form button[type="submit"]') as HTMLButtonElement
    assert.equal(submit.disabled, false)
    await act(async () => submit.click())

    assert.equal(button('Cancel').disabled, true)
    assert.equal(input('Import connection').disabled, true)
    assert.equal(input('Import artifact ID').disabled, true)
    assert.equal(input('Import revision ID').disabled, true)

    await act(async () => resolveImport({
      artifact_id: 'artifact-1',
      committed_library_version: 2,
      published_library_version: 1,
      new_generation: 1,
      relist_required: true,
      relist_guidance: 'refresh',
    }))
    assert.doesNotMatch(view.container.textContent ?? '', /Import exact Artifact/)
  } finally {
    skillLibrary.list = originalList
    skillLibrary.import = originalImport
    await view.unmount()
  }
})

test('refreshing the library does not lock a new-Skill editor to incidental selection', async () => {
  installTestDom()
  const originalList = skillLibrary.list
  const page: SkillLibraryPage = {
    library_version: 1,
    published_library_version: 1,
    can_create: true,
    create_visibilities: ['private'],
    allowed_actions: [],
    items: [item('alpha')],
  }
  skillLibrary.list = async () => page

  const view = await renderClient(<SkillLibraryPageContent />)
  try {
    await act(async () => {})
    const button = (label: string) => [...view.container.querySelectorAll('button')]
      .find(candidate => candidate.textContent?.includes(label)) as HTMLButtonElement
    await act(async () => button('Create skill').click())
    await act(async () => button('Refresh').click())

    const name = view.container.querySelector('input[aria-label="Skill name"]') as HTMLInputElement
    assert.equal(name.disabled, false)
    assert.equal(name.value, 'my-skill')
    assert.match(view.container.textContent ?? '', /Create a Skill/)
  } finally {
    skillLibrary.list = originalList
    await view.unmount()
  }
})
