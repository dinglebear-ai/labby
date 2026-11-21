'use client'

import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import type { GatewayRegistrationAdapter } from '@/lib/types/gateway'

export const emptyAdapterDraft = {
  manifestPath: '',
  specPath: '',
  baseUrl: '',
  operations: '',
  allowWrite: false,
}

type AdapterDraft = typeof emptyAdapterDraft
type AdapterKind = GatewayRegistrationAdapter['kind']

export function registrationAdapter(kind: AdapterKind, draft: AdapterDraft): GatewayRegistrationAdapter | null {
  const isAbsolute = (path: string) => path.startsWith('/') || /^[A-Za-z]:[\\/]/u.test(path)
  if (kind === 'cli') {
    const manifest_path = draft.manifestPath.trim()
    return isAbsolute(manifest_path) ? { kind, manifest_path } : null
  }

  const spec_path = draft.specPath.trim()
  if (!isAbsolute(spec_path) || !spec_path.endsWith('.json')) return null
  const base_url = draft.baseUrl.trim()
  try {
    const url = new URL(base_url)
    if (url.protocol !== 'https:' || url.username || url.password || base_url.includes('?') || base_url.includes('#')) return null
  } catch {
    return null
  }
  const allowed_operations = draft.operations.split(/[\n,]/u).map((value) => value.trim()).filter(Boolean)
  if (
    allowed_operations.length === 0
    || allowed_operations.length > 200
    || new Set(allowed_operations).size !== allowed_operations.length
    || allowed_operations.some((id) => !/^[A-Za-z0-9_.-]{1,128}$/u.test(id))
  ) return null

  return { kind, spec_path, base_url, allowed_operations, ...(draft.allowWrite ? { allow_write: true } : {}) }
}

export function GatewayAdapterFields({
  kind,
  draft,
  onChange,
  disabled,
}: {
  kind: AdapterKind
  draft: AdapterDraft
  onChange: (next: AdapterDraft) => void
  disabled: boolean
}) {
  if (kind === 'cli') {
    return (
      <Field>
        <FieldLabel htmlFor="adapter-manifest-path">Manifest path on the gateway host</FieldLabel>
        <Input
          id="adapter-manifest-path"
          aria-describedby="adapter-manifest-help"
          value={draft.manifestPath}
          onChange={(event) => onChange({ ...draft, manifestPath: event.target.value })}
          disabled={disabled}
          placeholder="/srv/labby/cli/status.toml"
          autoComplete="off"
          spellCheck={false}
        />
        <FieldDescription id="adapter-manifest-help">
          Enter the absolute path to an approved CLI manifest already on the Labby host.
          An operator must configure its allowed directory and permit Labby&apos;s executable.
          This does not upload a file.
        </FieldDescription>
      </Field>
    )
  }

  return (
    <>
      <Field>
        <FieldLabel htmlFor="adapter-spec-path">Specification path on the gateway host</FieldLabel>
        <Input
          id="adapter-spec-path"
          aria-describedby="adapter-spec-help"
          value={draft.specPath}
          onChange={(event) => onChange({ ...draft, specPath: event.target.value })}
          disabled={disabled}
          placeholder="/srv/labby/openapi/catalog.json"
          autoComplete="off"
          spellCheck={false}
        />
        <FieldDescription id="adapter-spec-help">
          Enter an absolute path to an OpenAPI JSON file already on the Labby host.
          An operator must configure its allowed directory and permit the OpenAPI adapter executable.
          This does not upload a file.
        </FieldDescription>
      </Field>
      <Field>
        <FieldLabel htmlFor="adapter-base-url">API base URL</FieldLabel>
        <Input
          id="adapter-base-url"
          type="url"
          aria-describedby="adapter-base-url-help"
          value={draft.baseUrl}
          onChange={(event) => onChange({ ...draft, baseUrl: event.target.value })}
          disabled={disabled}
          placeholder="https://api.example.com"
          autoComplete="off"
          spellCheck={false}
        />
        <FieldDescription id="adapter-base-url-help">
          Use a public HTTPS URL without credentials, a query, or a fragment.
        </FieldDescription>
      </Field>
      <Field>
        <FieldLabel htmlFor="adapter-operations">Allowed operation IDs</FieldLabel>
        <Textarea
          id="adapter-operations"
          aria-describedby="adapter-operations-help"
          value={draft.operations}
          onChange={(event) => onChange({ ...draft, operations: event.target.value })}
          disabled={disabled}
          placeholder="listItems"
          autoComplete="off"
          spellCheck={false}
        />
        <FieldDescription id="adapter-operations-help">
          Enter at least one exact operationId from the spec, one per line or separated by commas. No duplicates.
        </FieldDescription>
      </Field>
      <Field orientation="horizontal">
        <div className="flex-1">
          <FieldLabel htmlFor="adapter-allow-write">Allow write operations</FieldLabel>
          <FieldDescription id="adapter-write-help">Off by default. Write operations must also be listed above.</FieldDescription>
        </div>
        <Switch
          id="adapter-allow-write"
          aria-describedby="adapter-write-help"
          checked={draft.allowWrite}
          onCheckedChange={(allowWrite) => onChange({ ...draft, allowWrite })}
          disabled={disabled}
        />
      </Field>
    </>
  )
}
