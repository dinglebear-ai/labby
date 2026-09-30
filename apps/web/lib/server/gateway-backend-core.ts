interface BackendErrorPayload {
  message?: string
  kind?: string
}

function withRequestId(message: string, requestId?: string) {
  return requestId ? `${message} (request id: ${requestId})` : message
}

export class BackendGatewayError extends Error {
  status: number
  kind?: string
  requestId?: string

  constructor(message: string, status: number, kind?: string, requestId?: string) {
    super(message)
    this.name = 'BackendGatewayError'
    this.status = status
    this.kind = kind
    this.requestId = requestId
  }
}

function gatewayBackendBaseUrl(): string {
  const baseUrl = process.env.LAB_GATEWAY_API_URL || 'http://127.0.0.1:8765/v1'
  return baseUrl.endsWith('/') ? baseUrl.slice(0, -1) : baseUrl
}

function gatewayBackendHeaders(): HeadersInit {
  const headers: HeadersInit = {
    'Content-Type': 'application/json',
  }
  if (process.env.LAB_GATEWAY_API_TOKEN) {
    headers.Authorization = `Bearer ${process.env.LAB_GATEWAY_API_TOKEN}`
  }
  return headers
}

async function parseBackendError(response: Response): Promise<BackendGatewayError> {
  const payload = (await response.json().catch(() => null)) as BackendErrorPayload | null
  const requestId = response.headers.get('x-request-id') ?? undefined
  return new BackendGatewayError(
    withRequestId(
      payload?.message || `Gateway backend request failed with ${response.status}`,
      requestId,
    ),
    response.status,
    payload?.kind,
    requestId,
  )
}

export async function gatewayAction<T>(action: string, params: object): Promise<T> {
  let response: Response
  try {
    response = await fetch(`${gatewayBackendBaseUrl()}/gateway`, {
      method: 'POST',
      headers: gatewayBackendHeaders(),
      body: JSON.stringify({ action, params }),
      cache: 'no-store',
    })
  } catch (error) {
    const message = error instanceof Error ? error.message : 'unknown network error'
    throw new BackendGatewayError(
      `Gateway backend action \`${action}\` failed before a response was received: ${message}`,
      502,
      'backend_unreachable',
    )
  }

  if (!response.ok) {
    throw await parseBackendError(response)
  }

  return response.json() as Promise<T>
}
