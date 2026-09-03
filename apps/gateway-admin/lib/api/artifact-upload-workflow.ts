export type ArtifactUploadStage = 'creating the upload slot' | 'uploading bytes' | 'starting ingestion'

export class ArtifactUploadWorkflowError extends Error {
  constructor(message: string, readonly stage: ArtifactUploadStage, readonly uploadId = '') {
    super(message)
    this.name = 'ArtifactUploadWorkflowError'
  }
}

export async function runArtifactUpload<T extends Record<string, unknown>>(input: {
  file: File
  namespace: string
  create: (filename: string) => Promise<T>
  uploadId: (created: T) => string
  putBytes: (uploadId: string, file: File) => Promise<unknown>
  startJob: (params: Record<string, unknown>) => Promise<T>
  onCreated: (created: T, uploadId: string) => void
}) {
  let stage: ArtifactUploadStage = 'creating the upload slot'
  let uploadId = ''
  try {
    const created = await input.create(input.file.name)
    uploadId = input.uploadId(created)
    if (!uploadId || uploadId === 'unknown') throw new Error('Authority did not return an upload id')
    input.onCreated(created, uploadId)
    stage = 'uploading bytes'
    await input.putBytes(uploadId, input.file)
    stage = 'starting ingestion'
    return await input.startJob({
      kind: input.file.name.endsWith('.json') ? 'marketplace' : 'archive',
      arguments: input.file.name.endsWith('.json')
        ? { uploadId, baseSource: input.file.name }
        : { uploadId, namespace: input.namespace },
      idempotency_key: `gateway-admin-upload-${crypto.randomUUID()}`,
    })
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : 'Artifact upload failed'
    throw new ArtifactUploadWorkflowError(message, stage, uploadId)
  }
}
