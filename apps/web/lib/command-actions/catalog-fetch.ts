import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import type { CatalogResponse } from '@/lib/types/command-catalog'

/**
 * Fetch and validate the /v1/catalog payload.
 *
 * Suitable for use as an SWR fetcher.  Throws on network error, non-2xx
 * response, or Zod validation failure.
 */
export async function fetchCatalog(signal?: AbortSignal): Promise<CatalogResponse> {
  const url = `${normalizeGatewayApiBase()}/catalog`

  const response = await fetch(url, {
    method: 'GET',
    credentials: 'include',
    cache: 'no-store',
    signal,
  })

  if (!response.ok) {
    const errorBody = await response.json().catch(() => null)
    const message =
      errorBody != null && typeof errorBody === 'object' && 'message' in errorBody
        ? String((errorBody as Record<string, unknown>).message)
        : `catalog fetch failed: ${response.status}`
    throw Object.assign(new Error(message), {
      kind: 'catalog_fetch_error',
      status: response.status,
    })
  }

  let raw: unknown
  try {
    raw = await response.json()
  } catch {
    throw new Error('decode_error: catalog response is not valid JSON')
  }

  // Keep schema code off the shell's initial path while validating every response
  // before its data can enter the palette cache.
  const { CatalogResponseSchema } = await import('./catalog-schema')
  const result = CatalogResponseSchema.safeParse(raw)
  if (!result.success) {
    throw Object.assign(
      new Error(`catalog response failed validation: ${result.error.message}`),
      { kind: 'decode_error', zodError: result.error },
    )
  }

  return result.data
}
