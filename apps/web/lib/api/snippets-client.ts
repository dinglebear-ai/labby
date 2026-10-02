import { snippetsActionUrl } from './gateway-config'
import { performServiceAction, type ServiceActionError } from './service-action-client'
import type {
  CodeModeExecutionResponse,
  CreateSnippetInput,
  ResolvedSnippet,
  SnippetInfo,
  SnippetArtifactResponse,
  SnippetHistoryResponse,
  SnippetExecutionReceipt,
  SnippetListResponse,
  SnippetRemoveResult,
  SnippetTestResult,
  SnippetValidation,
} from '@/lib/types/snippets'

export class SnippetsApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'SnippetsApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

async function snippetsAction<T>(action: string, params: object, signal?: AbortSignal): Promise<T> {
  return performServiceAction<T, SnippetsApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Snippets',
    url: snippetsActionUrl(),
    createError: (message, status, code, param) => new SnippetsApiError(message, status, code, param),
  })
}

export const snippetsApi = {
  artifact(executionId: string, path: string, signal?: AbortSignal): Promise<SnippetArtifactResponse> {
    return snippetsAction<SnippetArtifactResponse>('snippets.artifact', { execution_id: executionId, path }, signal)
  },

  history(params: { name?: string; limit?: number; cursor?: string } = {}, signal?: AbortSignal): Promise<SnippetHistoryResponse> {
    return snippetsAction<SnippetHistoryResponse>('snippets.history', params, signal)
  },

  receipt(executionId: string, signal?: AbortSignal): Promise<SnippetExecutionReceipt> {
    return snippetsAction<SnippetExecutionReceipt>('snippets.receipt', { execution_id: executionId }, signal)
  },

  listDetails(signal?: AbortSignal): Promise<SnippetListResponse> {
    return snippetsAction<SnippetListResponse>('snippets.list', {}, signal)
  },

  testOffline(
    name: string,
    params: Record<string, unknown> = {},
    fixture?: Record<string, unknown>,
    signal?: AbortSignal,
  ): Promise<SnippetTestResult> {
    return snippetsAction<SnippetTestResult>('snippets.test', { name, params, ...(fixture ? { fixture } : {}) }, signal)
  },

  async list(signal?: AbortSignal): Promise<SnippetInfo[]> {
    const response = await snippetsAction<SnippetListResponse>('snippets.list', {}, signal)
    return response.snippets
  },

  get(name: string, signal?: AbortSignal): Promise<ResolvedSnippet> {
    return snippetsAction<ResolvedSnippet>('snippets.get', { name }, signal)
  },

  create(input: CreateSnippetInput, signal?: AbortSignal): Promise<SnippetInfo> {
    return snippetsAction<SnippetInfo>('snippets.create', input, signal)
  },

  validate(name: string, body?: string, signal?: AbortSignal): Promise<SnippetValidation> {
    return snippetsAction<SnippetValidation>(
      'snippets.validate',
      body === undefined ? { name } : { name, body },
      signal,
    )
  },

  testLive(name: string, params: Record<string, unknown> = {}, signal?: AbortSignal): Promise<SnippetTestResult> {
    return snippetsAction<SnippetTestResult>('snippets.test', { name, params, live: true }, signal)
  },

  testAllLive(signal?: AbortSignal): Promise<SnippetTestResult> {
    return snippetsAction<SnippetTestResult>('snippets.test', { all: true, live: true }, signal)
  },

  exec(
    name: string,
    params: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<CodeModeExecutionResponse> {
    return snippetsAction<CodeModeExecutionResponse>('snippets.exec', { name, params }, signal)
  },

  remove(name: string, signal?: AbortSignal): Promise<SnippetRemoveResult> {
    return snippetsAction<SnippetRemoveResult>('snippets.remove', { name }, signal)
  },
}
