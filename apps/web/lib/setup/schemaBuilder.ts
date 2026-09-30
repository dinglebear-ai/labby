// Build a Zod schema from a list of UiFieldSchema entries (lab-bg3e.1).
// The builder is pure — given the same field list it returns an
// equivalent schema, so callers can memoize it on a stable
// `schemaVersion` key.
//
// A `schemaVersion` helper is exported so consumers (e.g. ServiceForm)
// have a stable cache key even when the field array reference changes
// across renders.

import { z, type ZodTypeAny } from 'zod'

import type { ServiceEnvVar, UiFieldSchema } from '@/lib/api/setup-client'

export interface FieldView {
  name: string
  description: string
  example: string
  required: boolean
  secret: boolean
  ui: UiFieldSchema
  /** True when the draft already holds a non-blank value for this key. */
  hasStoredSecret?: boolean
}

/** Default UI shape used when an EnvVar has no explicit `ui` projection. */
export const DEFAULT_UI: UiFieldSchema = {
  kind: 'text',
  enum_values: null,
  advanced: false,
  help_url: null,
  depends_on: null,
  validation: {
    required: false,
    min_length: null,
    max_length: null,
    pattern: null,
  },
}

/** Lift the API's `ServiceEnvVar` shape into the form-rendering view. */
export function envVarToFieldView(
  envVar: ServiceEnvVar,
  hasStoredSecret = false,
): FieldView {
  return {
    name: envVar.name,
    description: envVar.description,
    example: envVar.example,
    required: envVar.required,
    secret: envVar.secret,
    ui: envVar.ui ?? DEFAULT_UI,
    hasStoredSecret: envVar.secret && hasStoredSecret,
  }
}

/**
 * Stable hash of a list of fields suitable for memoizing the resolver.
 * Hashes name + kind + advanced + validation only — value changes don't
 * matter to the schema shape.
 */
export function schemaVersion(fields: readonly FieldView[]): string {
  return fields
    .map((f) => {
      const v = f.ui.validation
      const enumPart = f.ui.kind === 'enum'
        ? `[${(f.ui.enum_values ?? []).join(',')}]`
        : ''
      return [
        f.name,
        f.ui.kind,
        enumPart,
        f.required ? 'R' : 'O',
        v.required ? '1' : '0',
        v.min_length ?? '-',
        v.max_length ?? '-',
        v.pattern ?? '-',
        f.hasStoredSecret ? 'S' : '',
      ].join('|')
    })
    .join(';')
}

// http: or https: only — `z.string().url()` accepts javascript:, data:, file:
// which are XSS sinks if the URL is ever rendered as an `<a href>`.
export const HTTPS_SCHEME_RE = /^https?:\/\//i

// Reject URL-encoded path separators / parent refs as well as raw `..`.
function isSafeRelativePath(val: string): boolean {
  if (val === '') return true
  // Absolute paths (POSIX `/foo`, Windows `C:\foo`) are not allowed in the
  // form. The Rust env_merge backend is the authoritative validator; this is
  // defense in depth for the operator typing into the wizard.
  if (val.startsWith('/') || val.startsWith('\\') || /^[A-Za-z]:/.test(val)) {
    return false
  }
  const lowered = val.toLowerCase()
  // URL-encoded traversal / null sequences (%2e%2e, %2f, %5c, %00).
  if (
    lowered.includes('%2e') ||
    lowered.includes('%2f') ||
    lowered.includes('%5c') ||
    lowered.includes('%00')
  ) {
    return false
  }
  // Null byte injection (raw).
  if (val.includes('\0')) return false
  // Plain `..` segment.
  return !val.split(/[\\/]/).includes('..')
}

function fieldRule(field: FieldView): ZodTypeAny {
  const v = field.ui.validation
  const isRequired = field.required || v.required

  // Secrets that already exist server-side accept blank ("keep current").
  const secretBlankOk = field.hasStoredSecret

  let str = z.string()
  if (v.min_length != null) str = str.min(v.min_length)
  if (v.max_length != null) str = str.max(v.max_length)
  if (v.pattern) {
    try {
      str = str.regex(new RegExp(v.pattern))
    } catch {
      // Bad pattern in PluginMeta — skip the regex, the audit gate will catch it.
    }
  }

  switch (field.ui.kind) {
    case 'url': {
      // Apply the same scheme allowlist on every path. `z.string().url()`
      // accepts javascript:/data:/file:, so use an explicit regex instead.
      const base = str.refine(
        (val) => val === '' || HTTPS_SCHEME_RE.test(val),
        'Must be an http or https URL',
      )
      if (isRequired && !secretBlankOk) {
        return base.refine((val) => val.length > 0, 'Required')
      }
      return base
    }
    case 'bool': {
      // Optional bool fields default to '' (no entry yet) — accept blank
      // so the resolver doesn't fail on mount before the user toggles.
      const variants = [
        z.literal('true'),
        z.literal('false'),
        z.literal('1'),
        z.literal('0'),
      ] as const
      return isRequired
        ? z.union([...variants])
        : z.union([...variants, z.literal('')])
    }
    case 'number':
      return str.refine(
        (val) => val === '' || !Number.isNaN(Number(val)),
        'Must be a number',
      )
    case 'enum': {
      const values = field.ui.enum_values ?? []
      if (values.length === 0) return str
      return z.enum(values as [string, ...string[]])
    }
    case 'file_path':
      return str.refine(
        isSafeRelativePath,
        'Must be a relative path without traversal sequences',
      )
    case 'secret':
    case 'text':
    default:
      if (isRequired && !secretBlankOk) {
        return str.min(1, 'Required')
      }
      return str
  }
}

/** Produce a Zod object schema covering every field in `fields`. */
export function buildSchema(
  fields: readonly FieldView[],
): z.ZodObject<Record<string, ZodTypeAny>> {
  const shape: Record<string, ZodTypeAny> = {}
  for (const field of fields) {
    shape[field.name] = fieldRule(field)
  }
  return z.object(shape)
}

/** Strip blank secret values so they don't overwrite stored "***" sentinels. */
export function stripBlankSecrets(
  values: Record<string, string>,
  fields: readonly FieldView[],
): Record<string, string> {
  const out: Record<string, string> = {}
  for (const field of fields) {
    const value = values[field.name]
    if (field.hasStoredSecret && (value === undefined || value === '')) {
      continue
    }
    if (value !== undefined) {
      out[field.name] = value
    }
  }
  return out
}
