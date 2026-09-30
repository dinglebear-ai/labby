'use client'

// ServiceForm — schema-rendered configuration form for one Bootstrap
// service. Shared between the lab-bg3e.4 wizard and the lab-bg3e.5
// settings rail. Navigation-agnostic: the parent shell decides what
// happens after `onSave` resolves.
//
// Key design points (see lab-bg3e.4 locked decisions):
// - Resolver is memoized on a stable `schemaVersion` key so RHF's
//   internal cache stays warm across renders.
// - Async probe state lives OUTSIDE RHF so `formState.isValidating`
//   doesn't lock the form during a slow round-trip.
// - Secret fields render as `type="password"` with an Eye toggle and a
//   "Leave blank to keep current value" placeholder when the draft
//   already holds a value.
// - Advanced fields hide behind a single collapsible disclosure.

import { useEffect, useMemo, useRef, useState } from 'react'
import { Controller, useForm, type SubmitHandler } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { ChevronDown, ChevronUp, Eye, EyeOff, ExternalLink, Loader2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'

import {
  HTTPS_SCHEME_RE,
  buildSchema,
  schemaVersion,
  stripBlankSecrets,
  type FieldView,
} from '@/lib/setup/schemaBuilder'

export type ProbeStatus = 'idle' | 'pending' | 'ok' | 'fail'

export interface ProbeOutcome {
  status: 'ok' | 'fail'
  message?: string
}

export interface ServiceFormProps {
  /** Field projections from setup-client's ServiceSchema. */
  fields: readonly FieldView[]
  /** Initial values. Blank for unset; secret-stored fields receive
      empty string + the schemaBuilder marks them with `hasStoredSecret`. */
  defaultValues: Record<string, string>
  /** Called when the user clicks Save. Stripping blank secrets is
      already applied. The parent decides whether this maps to
      setup.draft.set, setup.draft.commit, or both. */
  onSave: (values: Record<string, string>) => Promise<void> | void
  /** Optional async probe (doctor.service.probe) called on blur once
      every required field is populated. The signal is aborted when a new
      probe supersedes this one or when the form unmounts; consumers must
      thread it into the underlying fetch. */
  onProbe?: (values: Record<string, string>, signal: AbortSignal) => Promise<ProbeOutcome>
  /** Called once on unmount with the form's last-known values. Used by
      the wizard's configuration tab so switching tabs doesn't lose
      in-progress edits. The callback fires in the useEffect cleanup, so
      it sees values that were typed but not yet submitted. */
  onUnmount?: (values: Record<string, string>) => void
  /** Submit button label. Defaults to "Save". */
  submitLabel?: string
  /** Disable the form (e.g. while a parent commit is in flight). */
  disabled?: boolean
}

export function ServiceForm({
  fields,
  defaultValues,
  onSave,
  onProbe,
  onUnmount,
  submitLabel = 'Save',
  disabled = false,
}: ServiceFormProps): React.ReactElement {
  // Stable cache key for the resolver. RHF rebuilds its internal field
  // map when the resolver identity changes, so memoization here is
  // load-bearing for performance — not a nice-to-have.
  const version = useMemo(() => schemaVersion(fields), [fields])
  // eslint-disable-next-line react-hooks/exhaustive-deps -- version is the shape-stable cache key for fields
  const resolver = useMemo(() => zodResolver(buildSchema(fields)), [version])

  const form = useForm<Record<string, string>>({
    resolver,
    defaultValues,
    mode: 'onBlur',
  })

  const [secretShown, setSecretShown] = useState<Record<string, boolean>>({})
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [probe, setProbe] = useState<{ status: ProbeStatus; message?: string }>({
    status: 'idle',
  })
  const probeAbortRef = useRef<AbortController | null>(null)

  // Abort any in-flight probe when the form unmounts. Without this the
  // probe's promise resolves after the component is gone and setProbe
  // logs to a dead state setter.
  useEffect(() => {
    return () => {
      probeAbortRef.current?.abort()
    }
  }, [])

  // Flush current values to the parent on unmount. Ref keeps the latest
  // callback identity without re-arming the cleanup effect.
  const onUnmountRef = useRef(onUnmount)
  onUnmountRef.current = onUnmount
  useEffect(() => {
    return () => {
      onUnmountRef.current?.(form.getValues())
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []) // empty deps — fires only on unmount

  const { visibleFields, advancedFields } = useMemo(() => ({
    visibleFields: fields.filter((f) => !f.ui.advanced),
    advancedFields: fields.filter((f) => f.ui.advanced),
  }), [fields])
  const hasAdvanced = advancedFields.length > 0

  // FieldRow is not React.memo-wrapped today, so callback identity stability
  // would not avoid any re-renders. Plain function: simpler, equivalent.
  const toggleSecret = (name: string): void => {
    setSecretShown((prev) => ({ ...prev, [name]: !prev[name] }))
  }

  const submit: SubmitHandler<Record<string, string>> = async (values) => {
    const stripped = stripBlankSecrets(values, fields)
    await onSave(stripped)
  }

  async function runProbe(): Promise<void> {
    if (!onProbe) return
    const values = form.getValues()
    const ready = fields
      .filter((f) => f.required)
      .every((f) => {
        const v = values[f.name]
        return f.hasStoredSecret || (typeof v === 'string' && v.length > 0)
      })
    if (!ready) {
      setProbe({ status: 'idle' })
      return
    }
    probeAbortRef.current?.abort()
    const controller = new AbortController()
    probeAbortRef.current = controller
    setProbe({ status: 'pending' })
    try {
      const outcome = await onProbe(stripBlankSecrets(values, fields), controller.signal)
      if (controller.signal.aborted) return
      setProbe({ status: outcome.status, message: outcome.message })
    } catch (err) {
      if (controller.signal.aborted) return
      setProbe({
        status: 'fail',
        message: err instanceof Error ? err.message : 'probe failed',
      })
    }
  }

  return (
    <form onSubmit={form.handleSubmit(submit)} className="flex flex-col gap-4">
      {visibleFields.map((field) => (
        <FieldRow
          key={field.name}
          field={field}
          form={form}
          secretShown={secretShown[field.name] ?? false}
          onToggleSecret={toggleSecret}
          onBlurProbe={runProbe}
          disabled={disabled}
        />
      ))}

      {hasAdvanced ? (
        <div className="border-t pt-4">
          <button
            type="button"
            onClick={() => setShowAdvanced((v) => !v)}
            className="flex items-center gap-1 text-sm font-medium text-aurora-text-muted hover:text-aurora-text-primary"
          >
            {showAdvanced ? <ChevronUp className="h-3 w-3" /> : <ChevronDown className="h-3 w-3" />}
            Advanced
          </button>
          {showAdvanced ? (
            <div className="mt-3 flex flex-col gap-4">
              {advancedFields.map((field) => (
                <FieldRow
                  key={field.name}
                  field={field}
                  form={form}
                  secretShown={secretShown[field.name] ?? false}
                  onToggleSecret={toggleSecret}
                  onBlurProbe={runProbe}
                  disabled={disabled}
                />
              ))}
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="flex items-center gap-2 pt-2">
        <Button type="submit" disabled={disabled || form.formState.isSubmitting}>
          {form.formState.isSubmitting ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : null}
          {submitLabel}
        </Button>

        {onProbe ? (
          <Button
            type="button"
            variant="outline"
            disabled={disabled || probe.status === 'pending'}
            onClick={runProbe}
          >
            {probe.status === 'pending' ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : null}
            Test connection
          </Button>
        ) : null}

        <ProbeStatusBadge status={probe.status} message={probe.message} />
      </div>
    </form>
  )
}

interface FieldRowProps {
  field: FieldView
  form: ReturnType<typeof useForm<Record<string, string>>>
  secretShown: boolean
  onToggleSecret: (name: string) => void
  onBlurProbe: () => void
  disabled: boolean
}

function FieldRow({
  field,
  form,
  secretShown,
  onToggleSecret,
  onBlurProbe,
  disabled,
}: FieldRowProps): React.ReactElement {
  const error = form.formState.errors[field.name]
  const errorMessage = typeof error?.message === 'string' ? error.message : undefined
  const placeholder = field.hasStoredSecret
    ? 'Leave blank to keep current value'
    : field.example
  const id = `field-${field.name}`

  if (field.ui.kind === 'bool') {
    return (
      <div className="flex items-center justify-between rounded-md border p-3">
        <div>
          <Label htmlFor={id} className="font-medium">
            {field.name}
          </Label>
          {field.description ? (
            <p className="text-sm text-aurora-text-muted">{field.description}</p>
          ) : null}
        </div>
        {/* Controller scopes the subscription to this field — `form.watch`
            in render would re-render the entire form on every keystroke
            in any field. */}
        <Controller
          control={form.control}
          name={field.name}
          render={({ field: f }) => (
            <Switch
              id={id}
              checked={f.value === 'true'}
              onCheckedChange={(checked) => f.onChange(checked ? 'true' : 'false')}
              disabled={disabled}
              aria-label={field.name}
            />
          )}
        />
      </div>
    )
  }

  if (field.ui.kind === 'enum') {
    return (
      <div className="flex flex-col gap-1">
        <FieldLabel field={field} htmlFor={id} />
        <select
          id={id}
          {...form.register(field.name, { onBlur: onBlurProbe })}
          disabled={disabled}
          className={cn(
            'border rounded-md p-2 text-sm bg-aurora-page-bg',
            errorMessage ? 'border-destructive' : 'border-input',
          )}
        >
          {(field.ui.enum_values ?? []).map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
        {errorMessage ? (
          <p className="text-xs text-destructive">{errorMessage}</p>
        ) : null}
      </div>
    )
  }

  const isSecret = field.secret || field.ui.kind === 'secret'
  const inputType = isSecret && !secretShown
    ? 'password'
    : field.ui.kind === 'number'
      ? 'number'
      : 'text'

  return (
    <div className="flex flex-col gap-1">
      <FieldLabel field={field} htmlFor={id} />
      <div className="relative">
        <Input
          id={id}
          type={inputType}
          placeholder={placeholder}
          disabled={disabled}
          aria-invalid={errorMessage ? true : undefined}
          // new-password tells password managers not to auto-fill from
          // saved passwords AND not to offer to save on submit. API keys
          // and tokens should never end up in cloud-synced password vaults.
          // Non-secret technical fields (URLs, file paths) opt out of
          // browser autofill suggestions, which are useless here.
          autoComplete={isSecret ? 'new-password' : 'off'}
          {...form.register(field.name, { onBlur: onBlurProbe })}
          className={cn(isSecret ? 'pr-10' : undefined,
            errorMessage ? 'border-destructive' : undefined,
          )}
        />
        {isSecret ? (
          <button
            type="button"
            aria-label={secretShown ? 'Hide secret' : 'Show secret'}
            onClick={() => onToggleSecret(field.name)}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-aurora-text-muted hover:text-aurora-text-primary"
          >
            {secretShown ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
          </button>
        ) : null}
      </div>
      {errorMessage ? (
        <p className="text-xs text-destructive">{errorMessage}</p>
      ) : null}
    </div>
  )
}

function FieldLabel({
  field,
  htmlFor,
}: {
  field: FieldView
  htmlFor: string
}): React.ReactElement {
  return (
    <Label htmlFor={htmlFor} className="flex items-center gap-1 font-mono text-xs">
      <span>{field.name}</span>
      {field.required ? <span className="text-destructive">*</span> : null}
      {/* Defense in depth: even though help_url comes from the schema (not
          user input), validate the scheme so a tampered schema response
          can't slip a javascript:/data: URI past noopener noreferrer. */}
      {field.ui.help_url && HTTPS_SCHEME_RE.test(field.ui.help_url) ? (
        <a
          href={field.ui.help_url}
          target="_blank"
          rel="noopener noreferrer"
          className="text-aurora-text-muted hover:text-aurora-text-primary"
          aria-label={`Help for ${field.name}`}
        >
          <ExternalLink className="h-3 w-3" />
        </a>
      ) : null}
      {field.description ? (
        <span className="ml-2 font-sans text-aurora-text-muted normal-case">
          — {field.description}
        </span>
      ) : null}
    </Label>
  )
}

function ProbeStatusBadge({
  status,
  message,
}: {
  status: ProbeStatus
  message?: string
}): React.ReactElement | null {
  if (status === 'idle') return null
  if (status === 'pending') {
    return (
      <span className="text-xs text-aurora-text-muted inline-flex items-center gap-1">
        <Loader2 className="h-3 w-3 animate-spin" /> probing
      </span>
    )
  }
  if (status === 'ok') {
    return <span className="text-xs text-emerald-600">✓ reachable</span>
  }
  return (
    <span className="text-xs text-destructive">
      ✗ {message ?? 'unreachable'}
    </span>
  )
}

export default ServiceForm
