import test from 'node:test'
import assert from 'node:assert/strict'
import { localLibraryArtifact } from './local-library-model'
import type { SkillLibraryItem } from '@/lib/api/skill-library-client'

test('local acquired records preserve identity and revision without inventing remote metadata', () => {
  const item: SkillLibraryItem = { artifact_id: 'artifact-one', name: 'Review', archived: false, latest_revision_id: 'revision-two', visibility: 'private', access_label: 'Personal', can_mutate: true, owner: { relationship: 'owner' }, provenance: { source: 'depot' }, materialized: true, current_generation: 2, published_library_version: 7, allowed_actions: ['artifacts.read'], latest_revision_files: [{ path: 'SKILL.md', digest: 'sha256:abc', size: 42, media_type: 'text/markdown' }] }
  assert.deepEqual(localLibraryArtifact(item), { id: 'artifact-one', name: 'Review', kind: 'skill', namespace: 'Personal', currentRevisionId: 'revision-two', currentRevision: { id: 'revision-two', components: [{ path: 'SKILL.md', mediaType: 'text/markdown', size: 42 }] }, publication: { visibility: 'private' } })
})
