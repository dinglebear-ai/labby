import { getBrowserSessionContextIdentity } from '../auth/session-store.ts'

export type ArtifactUploadStage = 'creating the upload slot' | 'uploading bytes' | 'starting ingestion'

export class ArtifactUploadWorkflowError extends Error {
  constructor(message: string, readonly stage: ArtifactUploadStage, readonly uploadId = '') {
    super(message)
    this.name = 'ArtifactUploadWorkflowError'
  }
}

export async function runArtifactUpload<TCreated extends Record<string, unknown>, TResult>(input: {
  file: File
  namespace: string
  create: (filename: string) => Promise<TCreated>
  uploadId: (created: TCreated) => string
  putBytes: (uploadId: string, file: File) => Promise<unknown>
  startJob: (params: Record<string, unknown>) => Promise<TResult>
  onCreated: (created: TCreated, uploadId: string) => void
}) {
  const context = getBrowserSessionContextIdentity()
  const assertCurrentContext = () => {
    if (context !== getBrowserSessionContextIdentity()) throw new DOMException('Authority or project context changed', 'AbortError')
  }
  let stage: ArtifactUploadStage = 'creating the upload slot'
  let uploadId = ''
  try {
    const created = await input.create(input.file.name)
    assertCurrentContext()
    uploadId = input.uploadId(created)
    if (!uploadId || uploadId === 'unknown') throw new Error('Authority did not return an upload id')
    input.onCreated(created, uploadId)
    assertCurrentContext()
    stage = 'uploading bytes'
    await input.putBytes(uploadId, input.file)
    assertCurrentContext()
    stage = 'starting ingestion'
    const result = await input.startJob({
      kind: input.file.name.endsWith('.json') ? 'marketplace' : 'archive',
      arguments: input.file.name.endsWith('.json')
        ? { uploadId, baseSource: input.file.name }
        : { uploadId, namespace: input.namespace },
      idempotency_key: `gateway-admin-upload-${crypto.randomUUID()}`,
    })
    assertCurrentContext()
    return result
  } catch (cause) {
    const message = cause instanceof Error ? cause.message : 'Artifact upload failed'
    throw new ArtifactUploadWorkflowError(message, stage, uploadId)
  }
}
