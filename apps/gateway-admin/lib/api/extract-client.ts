import { extractActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

export interface ExtractRuntimeMeta {
  container_name?: string
  image?: string | null
}

export interface ExtractCredential {
  service: string
  url: string | null
  env_field: string
  secret_present: boolean
  source_host?: string
  probe_host?: string
  runtime?: ExtractRuntimeMeta
  url_verified?: boolean
}

export interface ExtractWarning {
  service?: string
  host?: string
  runtime?: ExtractRuntimeMeta
  message: string
}

export interface ExtractSshUri {
  host: string
  path: string
}

export interface ExtractTarget {
  mode: 'fleet' | 'targeted'
  uri?: string | ExtractSshUri
}

export interface ExtractReport {
  target: ExtractTarget
  found: string[]
  creds: ExtractCredential[]
  warnings: ExtractWarning[]
}

export class ExtractApiError extends Error implements ServiceActionError {
  status: number
  code?: string

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'ExtractApiError'
    this.status = status
    this.code = code
  }
}

async function extractAction<T>(
  action: string,
  params: object,
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, ExtractApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Extract',
    url: extractActionUrl(),
    createError: (message, status, code) => new ExtractApiError(message, status, code),
  })
}

export const extractApi = {
  async listHosts(signal?: AbortSignal): Promise<string[]> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return ['lab-node-1', 'media-node']
    }
    return extractAction<string[]>('list_hosts', {}, signal)
  },

  async scan(opts?: {
    uri?: string
    hosts?: string[]
    signal?: AbortSignal
  }): Promise<ExtractReport> {
    if (USE_MOCK_DATA) {
      opts?.signal?.throwIfAborted?.()
      const target: ExtractTarget = opts?.uri
        ? { mode: 'targeted', uri: opts.uri }
        : { mode: 'fleet' }
      return {
        target,
        found: ['gateway_alpha', 'hidden_upstream'],
        creds: [
          {
            service: 'gateway_alpha',
            url: 'http://gateway_alpha.local:7878',
            env_field: 'GATEWAY_ALPHA_URL',
            secret_present: true,
            source_host: 'media-node',
            probe_host: 'gateway_alpha.local',
            runtime: { container_name: 'gateway_alpha', image: 'lscr.io/linuxserver/gateway_alpha:latest' },
            url_verified: true,
          },
          {
            service: 'hidden_upstream',
            url: 'http://hidden_upstream.local:8989',
            env_field: 'HIDDEN_UPSTREAM_URL',
            secret_present: true,
            source_host: 'media-node',
            probe_host: 'hidden_upstream.local',
            runtime: { container_name: 'hidden_upstream', image: 'lscr.io/linuxserver/hidden_upstream:latest' },
            url_verified: true,
          },
        ],
        warnings: [
          {
            service: 'gateway_beta',
            host: 'media-node',
            message: 'Mock scan found a Gateway beta container but no token in mounted config.',
          },
        ],
      }
    }
    const params: Record<string, unknown> = { redact_secrets: true }
    if (opts?.uri !== undefined) params.uri = opts.uri
    if (opts?.hosts !== undefined && opts.hosts.length > 0) params.hosts = opts.hosts
    return extractAction<ExtractReport>('scan', params, opts?.signal)
  },
}
