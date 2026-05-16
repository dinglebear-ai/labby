/**
 * Fetch function and Zod schema for the /v1/catalog endpoint.
 *
 * Parsing is done at the fetch boundary — never cast the raw response.
 * On parse failure a `decode_error` is thrown (consistent with docs/dev/ERRORS.md).
 */

import { z } from 'zod'

import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import type { CatalogResponse } from '@/lib/types/command-catalog'

// ── Zod schemas ──────────────────────────────────────────────────────────────

export const CatalogParamSchema = z.object({
  name: z.string(),
  ty: z.string(),
  required: z.boolean(),
  description: z.string(),
  secret: z.boolean().optional(),
})

/**
 * Rust's ActionEntry serialises the action name as `name`.
 * We normalise it to `action` at the fetch boundary so the palette
 * can use a single stable field name (`CatalogAction.action`).
 */
export const CatalogActionSchema = z.object({
  /** From Rust: ActionEntry::name (dotted action name, e.g. `movie.search`) */
  name: z.string(),
  description: z.string(),
  destructive: z.boolean(),
  params: z.array(CatalogParamSchema),
  returns: z.string(),
})
.transform(({ name, ...rest }) => ({ action: name, ...rest }))

export const CatalogServiceSchema = z.object({
  name: z.string(),
  description: z.string(),
  category: z.string().optional().default(''),
  status: z.string().optional().default('available'),
  actions: z.array(CatalogActionSchema),
})

export const CatalogResponseSchema = z.object({
  services: z.array(CatalogServiceSchema),
})

// ── Fetch function ────────────────────────────────────────────────────────────

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

  const result = CatalogResponseSchema.safeParse(raw)
  if (!result.success) {
    throw Object.assign(
      new Error(`catalog response failed validation: ${result.error.message}`),
      { kind: 'decode_error', zodError: result.error },
    )
  }

  return result.data
}
