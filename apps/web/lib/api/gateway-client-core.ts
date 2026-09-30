import { withRequestId } from '../http/request-id.ts'
import { gatewayActionUrl } from './gateway-config.ts'
import { gatewayRequestInit } from './gateway-request.ts'

export class GatewayApiError extends Error {
  status: number
  code?: string
  requestId?: string
  constructor(
    message: string,
    status: number,
    code?: string,
    requestId?: string,
  ) {
    super(message)
    this.name = 'GatewayApiError'
    this.status = status
    this.code = code
    this.requestId = requestId
  }
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === 'AbortError'
    : error instanceof Error && error.name === 'AbortError'
}

async function parseActionResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const requestId = response.headers.get('x-request-id') ?? undefined
    const error = await response.json().catch(() => ({ message: 'An error occurred' }))
    throw new GatewayApiError(
      withRequestId(error.message || 'An error occurred', requestId),
      response.status,
      error.kind || error.code,
      requestId,
    )
  }
  return response.json()
}

export async function gatewayActionCore<T>(
  action: string,
  params: object,
  signal?: AbortSignal,
): Promise<T> {
  let response: Response
  try {
    response = await fetch(gatewayActionUrl(), gatewayRequestInit(action, params, undefined, signal))
  } catch (error) {
    if (isAbortError(error)) {
      throw error
    }
    const message = error instanceof Error ? error.message : 'unknown network error'
    throw new GatewayApiError(
      `Gateway backend action \`${action}\` failed before a response was received: ${message}`,
      502,
      'backend_unreachable',
    )
  }

  return parseActionResponse<T>(response)
}
