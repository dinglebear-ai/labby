import test from 'node:test'
import assert from 'node:assert/strict'

import { ArtifactUploadWorkflowError, runArtifactUpload } from './artifact-upload-workflow.ts'

test('failed byte upload preserves the created slot and reports its exact stage', async () => {
  const created: string[] = []
  await assert.rejects(
    runArtifactUpload({
      file: new File(['bytes'], 'skill.zip'),
      namespace: 'imports',
      create: async () => ({ upload: { id: 'upload-1' } }),
      uploadId: (value) => (value.upload as { id: string }).id,
      putBytes: async () => { throw new Error('network down') },
      startJob: async () => ({ job: { id: 'unreachable' } }),
      onCreated: (_value, id) => created.push(id),
    }),
    (error: unknown) => error instanceof ArtifactUploadWorkflowError
      && error.stage === 'uploading bytes'
      && error.uploadId === 'upload-1',
  )
  assert.deepEqual(created, ['upload-1'])
})

test('failed job start identifies the uploaded slot for cleanup or inspection', async () => {
  await assert.rejects(
    runArtifactUpload({
      file: new File(['{}'], 'marketplace.json'),
      namespace: 'imports',
      create: async () => ({ upload: { id: 'upload-2' } }),
      uploadId: (value) => (value.upload as { id: string }).id,
      putBytes: async () => undefined,
      startJob: async () => { throw new Error('queue unavailable') },
      onCreated: () => undefined,
    }),
    (error: unknown) => error instanceof ArtifactUploadWorkflowError
      && error.stage === 'starting ingestion'
      && error.uploadId === 'upload-2',
  )
})
