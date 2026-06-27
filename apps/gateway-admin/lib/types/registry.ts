// TypeScript types mirroring crates/lab-apis/src/mcpregistry/types.rs
// JSON field names follow serde rename annotations in the Rust source.

export const REGISTRY_META_KEY = 'io.modelcontextprotocol.registry/official'
export const LAB_REGISTRY_META_KEY = 'dev.labby/registry'

export interface ListServersParams {
  search?: string
  owner?: string
  limit?: number
  cursor?: string | null
  version?: string
  updated_since?: string
  featured?: boolean
  reviewed?: boolean
  recommended?: boolean
  hidden?: boolean
  tag?: string
}

export interface ServerListResponse {
  servers: ServerResponse[]
  metadata: ListMetadata
}

export interface ListMetadata {
  count: number
  nextCursor?: string | null
}

export interface ServerResponse {
  server: NormalizedServerJSON
  _meta?: ResponseMeta | null
}

/// `ServerJSON` with array fields guaranteed non-null (normalized at the API boundary).
export type NormalizedServerJSON = Omit<ServerJSON, 'packages' | 'remotes' | 'icons'> & {
  packages: Package[]
  remotes: Transport[]
  icons: Icon[]
}

export function normalizeServerJSON(server: ServerJSON): NormalizedServerJSON {
  return {
    ...server,
    packages: server.packages ?? [],
    remotes: server.remotes ?? [],
    icons: server.icons ?? [],
  }
}

export interface ResponseMeta {
  'io.modelcontextprotocol.registry/official'?: RegistryExtensions | null
  'dev.labby/registry'?: LabRegistryMetadata | null
  [key: string]: unknown
}

export interface LabRegistryMetadata {
  curation?: {
    featured?: boolean
    hidden?: boolean
    tags?: string[]
    notes?: string | null
  } | null
  trust?: {
    reviewed?: boolean
    reviewed_at?: string | null
    source_verified?: boolean
    maintainer_known?: boolean
  } | null
  quality?: {
    install_tested?: boolean
    last_install_tested_at?: string | null
    transport_score?: 'good' | 'mixed' | 'poor' | null
  } | null
  security?: {
    ssrf_reviewed?: boolean
    permissions_reviewed?: boolean
    secrets_reviewed?: boolean
  } | null
  ux?: {
    works_in_lab?: boolean
    recommended_for_homelab?: boolean
    setup_difficulty?: 'easy' | 'medium' | 'hard' | null
  } | null
  audit?: {
    updated_at?: string | null
    updated_by?: string | null
  } | null
  extra?: Record<string, unknown> | null
  [key: string]: unknown
}

export interface RegistryExtensions {
  isLatest: boolean
  publishedAt: string
  status: 'active' | 'deprecated' | 'deleted'
  statusChangedAt: string
  statusMessage?: string | null
  updatedAt?: string | null
}

export interface ServerJSON {
  $schema?: string | null
  name: string
  title?: string | null
  description: string
  version: string
  packages?: Package[]
  remotes?: Transport[]
  repository?: Repository | null
  icons?: Icon[]
  websiteUrl?: string | null
}

export interface Transport {
  type: 'streamable-http' | 'sse' | 'stdio' | string
  url?: string | null
  headers?: Header[] | null
  variables?: Record<string, unknown> | null
}

export interface Header {
  name: string
  value: string
}

export interface Package {
  registryType: string
  identifier: string
  version?: string | null
  transport: Transport
  runtimeHint?: string | null
  runtimeArguments?: RuntimeArgument[] | null
  packageArguments?: PackageArgument[] | null
  environmentVariables?: EnvironmentVariable[] | null
  fileSha256?: string | null
  registryBaseUrl?: string | null
}

export interface RuntimeArgument {
  name: string
  value?: string | null
}

export interface PackageArgument {
  value: string
  description?: string | null
  isRequired: boolean
}

export interface EnvironmentVariable {
  name: string
  description?: string | null
  required: boolean
}

export interface Repository {
  url: string
  source?: string | null
  id?: string | null
  subfolder?: string | null
}

export interface Icon {
  src: string
  mimeType?: string | null
  sizes?: string[]
  theme?: string | null
}

export interface ValidationResult {
  valid: boolean
  issues: ValidationIssue[]
}

export interface ValidationIssue {
  field?: string | null
  message: string
  severity?: string | null
}

export interface RegistryLocalMetaResponse {
  name: string
  version: string
  namespace: string
  metadata: LabRegistryMetadata | null
}

export interface RegistryMetaSetOptions {
  version?: string
  updated_by?: string
}

export interface RegistryLocalMetaDeleteResponse {
  name: string
  version: string
  namespace: string
  deleted: boolean
}

export class RegistryApiError extends Error implements RegistryError {
  status: number
  code?: string

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'RegistryApiError'
    this.status = status
    this.code = code
  }
}

export interface RegistryError {
  message: string
  status: number
  code?: string
}
