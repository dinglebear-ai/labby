import { z } from 'zod'

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
