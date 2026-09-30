// Single source of truth for the masked-secret sentinel returned by the
// Rust setup dispatch. The backend redacts secret values to STORED_SECRET_MARKER
// in setup.draft.get responses; the UI must (a) never show this as a real
// value in form fields and (b) treat blank user input as "keep current"
// when the marker is present.

import type { DraftEntry, ServiceEnvVar } from '@/lib/api/setup-client'
import { envVarToFieldView, type FieldView } from '@/lib/setup/schemaBuilder'

export const STORED_SECRET_MARKER = '***'

export function isStoredSecret(value: string | undefined): boolean {
  return value === STORED_SECRET_MARKER
}

/** Convert a raw draft value to a form-renderable value. Masked secrets become
 * blank so the "leave blank to keep current" placeholder applies. */
export function unmaskValue(value: string | undefined): string {
  if (!value || isStoredSecret(value)) return ''
  return value
}

export function draftEntriesToMap(entries: ReadonlyArray<DraftEntry>): Record<string, string> {
  const map: Record<string, string> = {}
  for (const entry of entries) map[entry.key] = entry.value
  return map
}

/** Build the FieldView list and default-values map for a service form, given
 * the schema env list and the draft snapshot. Centralizes the secret-masking
 * contract that was previously duplicated across configuration/page.tsx and
 * settings/services/[service]/service-client.tsx. */
export function buildServiceFormDefaults(
  envVars: ReadonlyArray<ServiceEnvVar>,
  draftMap: Record<string, string>,
): { fields: FieldView[]; defaults: Record<string, string> } {
  const fields = envVars.map((envVar) =>
    envVarToFieldView(envVar, isStoredSecret(draftMap[envVar.name])),
  )
  const defaults: Record<string, string> = {}
  for (const field of fields) {
    defaults[field.name] = unmaskValue(draftMap[field.name])
  }
  return { fields, defaults }
}
