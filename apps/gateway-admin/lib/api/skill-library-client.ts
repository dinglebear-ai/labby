import { normalizeGatewayApiBase } from './gateway-config'
import { performServiceAction, type ServiceActionError } from './service-action-client'

export type SkillVisibility = 'private' | 'shared'

export interface SkillLibraryFile {
  path: string
  content: string
}

export interface SkillRevisionFile {
  path: string
  digest: string
  size: number
  media_type?: string | null
}

export interface SkillLibraryItem {
  artifact_id: string
  name: string
  archived: boolean
  active_revision_id?: string | null
  latest_revision_id: string
  visibility: SkillVisibility
  access_label: string
  can_mutate: boolean
  owner: { relationship: string }
  provenance: { source: string }
  materialized: boolean
  canonical_uri?: string | null
  current_generation: number
  published_library_version: number
  allowed_actions: string[]
  latest_revision_files: SkillRevisionFile[]
}

export interface SkillLibraryPage {
  library_version: number
  published_library_version: number
  can_create: boolean
  create_visibilities: SkillVisibility[]
  allowed_actions: string[]
  items: SkillLibraryItem[]
  next_cursor?: string | null
}

export interface SkillValidation {
  valid: boolean
  artifact_id?: string | null
  revision_id?: string | null
  rejections: Array<{ field: string; code: string; path?: string | null }>
}

export interface SkillMutationReceipt {
  artifact_id: string
  active_revision_id?: string | null
  committed_library_version: number
  published_library_version: number
  new_generation: number
  relist_required: boolean
  relist_guidance: string
}

export type SkillImportSource =
  | { kind: 'depot'; connection_id: string; artifact_id: string; revision_id: string }
  | { kind: 'repository'; connection_id: string; artifact_id: string; revision_id: string }

export interface SkillRevisionContents {
  library_version: number
  artifact_id: string
  revision_id: string
  path: string
  text: string
}

export class SkillLibraryApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'SkillLibraryApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

function skillsAction<T>(action: string, params: object, signal?: AbortSignal) {
  return performServiceAction<T, SkillLibraryApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Artifact Library',
    url: `${normalizeGatewayApiBase()}/artifacts`,
    createError: (message, status, code, param) =>
      new SkillLibraryApiError(message, status, code, param),
  })
}

export const skillLibrary = {
  list(query = '', signal?: AbortSignal) {
    const normalized = query.trim()
    return skillsAction<SkillLibraryPage>(normalized ? 'artifacts.search' : 'artifacts.list', {
      ...(normalized ? { query: normalized } : {}),
      limit: 100,
    }, signal)
  },
  validate(name: string, files: SkillLibraryFile[]) {
    return skillsAction<SkillValidation>('artifacts.validate', { name, files })
  },
  create(input: {
    name: string
    files: SkillLibraryFile[]
    visibility: SkillVisibility
    expectedLibraryVersion: number
    idempotencyKey: string
  }) {
    return skillsAction<SkillMutationReceipt>('artifacts.create', {
      name: input.name,
      files: input.files,
      visibility: input.visibility,
      expected_library_version: input.expectedLibraryVersion,
      idempotency_key: input.idempotencyKey,
    })
  },
  import(input: {
    source: SkillImportSource
    expectedLibraryVersion: number
    idempotencyKey: string
  }) {
    return skillsAction<SkillMutationReceipt>('artifacts.import', {
      source: input.source,
      expected_library_version: input.expectedLibraryVersion,
      idempotency_key: input.idempotencyKey,
    })
  },
  read(artifactId: string, revisionId: string, path: string) {
    return skillsAction<SkillRevisionContents>('artifacts.read', {
      artifact_id: artifactId,
      revision_id: revisionId,
      path,
    })
  },
  save(input: {
    artifactId: string
    revisionId: string
    files: SkillLibraryFile[]
    expectedLibraryVersion: number
    idempotencyKey: string
  }) {
    return skillsAction<SkillMutationReceipt>('artifacts.save', {
      artifact_id: input.artifactId,
      expected_revision_id: input.revisionId,
      files: input.files,
      expected_library_version: input.expectedLibraryVersion,
      idempotency_key: input.idempotencyKey,
    })
  },
  activate(input: {
    artifactId: string
    revisionId: string
    expectedLibraryVersion: number
    idempotencyKey: string
  }) {
    return skillsAction<SkillMutationReceipt>('artifacts.activate', {
      artifact_id: input.artifactId,
      expected_revision_id: input.revisionId,
      expected_library_version: input.expectedLibraryVersion,
      idempotency_key: input.idempotencyKey,
    })
  },
  archive(input: {
    artifactId: string
    expectedLibraryVersion: number
    idempotencyKey: string
  }) {
    return skillsAction<SkillMutationReceipt>('artifacts.archive', {
      artifact_id: input.artifactId,
      expected_library_version: input.expectedLibraryVersion,
      idempotency_key: input.idempotencyKey,
    })
  },
}
