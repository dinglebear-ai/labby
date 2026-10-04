'use client'

import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  describeCodeModeTool,
  type ToolDescription,
} from '@/lib/api/tool-browser-client'
import {
  guidedSchemaSupported,
  mappedParameterError,
  objectSchema,
  type ParameterSchema,
} from './tool-parameter-model'

export function ToolParameterForm({
  tool,
  declaredInputs = {},
  index,
  value,
  inputs,
  onChange,
  onSchema,
  autoLoad = false,
  descriptionLoader = describeCodeModeTool,
  onDescription,
  sources = [],
  suggestions = {},
  onRefresh,
  onAddInput,
}: {
  tool: string
  declaredInputs?: Record<
    string,
    import('@/lib/types/snippets').SnippetInputSpec
  >
  index: number
  value: string
  inputs: Record<string, unknown>
  onChange: (value: string) => void
  onSchema: (schema: ParameterSchema) => void
  autoLoad?: boolean
  descriptionLoader?: (
    tool: string,
    signal?: AbortSignal,
  ) => Promise<ToolDescription>
  onDescription?: (description: ToolDescription) => void
  sources?: Array<{ value: string; label: string }>
  suggestions?: Record<string, unknown>
  onRefresh?: () => void
  onAddInput?: (name: string, schema: ParameterSchema) => void
}) {
  const [schema, setSchema] = useState<ParameterSchema>()
  const [advanced, setAdvanced] = useState(true)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string>()
  const callbacks = useRef({ onSchema, onDescription })
  callbacks.current = { onSchema, onDescription }
  const load = async (signal?: AbortSignal) => {
    setLoading(true)
    setError(undefined)
    try {
      const description = await descriptionLoader(tool, signal)
      if (signal?.aborted) return
      callbacks.current.onDescription?.(description)
      const next = objectSchema(description.input_schema)
      if (!next)
        throw new Error(
          description.input_schema_omitted === 'size_limit'
            ? 'The tool schema exceeds the describe budget. Use Advanced JSON and verify the full schema in Tools.'
            : 'This tool does not expose an object input schema. Use JSON parameters and verify its schema in Tools.',
        )
      setSchema(next)
      callbacks.current.onSchema(next)
      setAdvanced(!guidedSchemaSupported(next))
    } catch (error) {
      if (!signal?.aborted)
        setError(
          error instanceof Error
            ? error.message
            : 'Unable to load tool schema.',
        )
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }
  useEffect(() => {
    if (!autoLoad || !tool) return
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
    // Loader is coalesced by the workflow editor and keyed to authority.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoLoad, tool, descriptionLoader])
  let mapping: Record<string, unknown> = {}
  try {
    const parsed = JSON.parse(value)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed))
      mapping = parsed
  } catch {
    /* Keep the advanced editor available for repair. */
  }
  const change = (name: string, next: unknown) => {
    const entries = Object.entries(mapping).filter(([key]) => key !== name)
    if (next !== undefined) entries.push([name, next])
    onChange(JSON.stringify(Object.fromEntries(entries), null, 2))
  }
  return (
    <div className="grid gap-2 rounded-aurora-2 border border-aurora-border-subtle p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Label htmlFor={`tool-mapping-${index}`}>Parameters for {tool}</Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={loading}
          onClick={() => {
            onRefresh?.()
            void load()
          }}
        >
          {loading ? 'Loading schema…' : 'Load tool schema'}
        </Button>
      </div>
      {error ? (
        <p role="alert" className="text-xs text-aurora-error">
          {error}
        </p>
      ) : null}
      {schema ? (
        <>
          <p className="text-xs text-aurora-text-muted">
            {schema.description ||
              'Parameters from the current tool schema. Required fields are marked *.'}
          </p>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={!guidedSchemaSupported(schema)}
            onClick={() => setAdvanced(!advanced)}
          >
            {advanced ? 'Use parameter form' : 'Advanced JSON'}
          </Button>
          {!guidedSchemaSupported(schema) ? (
            <p className="text-xs text-aurora-text-muted">
              This schema uses complex types. Edit JSON; the backend validates
              the complete schema.
            </p>
          ) : null}
        </>
      ) : null}
      {schema &&
      mappedParameterError(schema, mapping, inputs, declaredInputs) ? (
        <p className="text-xs text-aurora-error">
          {mappedParameterError(schema, mapping, inputs, declaredInputs)}
        </p>
      ) : null}
      {Object.keys(suggestions).length ? (
        <div className="grid gap-1 rounded-aurora-1 border border-aurora-border-subtle p-2 text-xs">
          <strong>
            Suggested mappings · matching name/type or declared default
          </strong>
          {Object.entries(suggestions)
            .filter(
              ([name]) => !Object.prototype.hasOwnProperty.call(mapping, name),
            )
            .map(([name, suggested]) => (
              <Button
                key={name}
                type="button"
                size="sm"
                variant="outline"
                className="h-auto justify-start whitespace-normal text-left"
                onClick={() => change(name, suggested)}
              >
                {name} ← {JSON.stringify(suggested)} ·{' '}
                {typeof suggested === 'string' && suggested.startsWith('$')
                  ? 'unique name/type match'
                  : 'schema default'}{' '}
                · Apply
              </Button>
            ))}
        </div>
      ) : null}
      {schema && !advanced ? (
        <div className="grid gap-3 sm:grid-cols-2">
          {Object.entries(schema.properties ?? {}).map(([name, property]) => {
            const id = `tool-${index}-${name}`
            const current = mapping[name]
            const reference =
              typeof current === 'string' && current.startsWith('$input.')
                ? current.slice(7)
                : undefined
            const outputReference =
              typeof current === 'string' && current.startsWith('$steps.')
                ? current
                : undefined
            const mode = reference
              ? `input:${reference}`
              : outputReference
                ? `output:${outputReference}`
                : 'constant'
            return (
              <div key={name} className="grid min-w-0 gap-1.5">
                <Label htmlFor={id}>
                  {name}
                  {schema.required?.includes(name) ? ' *' : ''}
                </Label>
                <Select
                  value={mode}
                  onValueChange={(mode) =>
                    change(
                      name,
                      mode.startsWith('input:')
                        ? `$input.${mode.slice(6)}`
                        : mode.startsWith('output:')
                          ? mode.slice(7)
                          : undefined,
                    )
                  }
                >
                  <SelectTrigger
                    aria-label={`Value source for ${tool} ${name}`}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="constant">Constant value</SelectItem>
                    {[
                      ...new Set([
                        ...Object.keys(inputs),
                        ...Object.keys(declaredInputs),
                      ]),
                    ].map((key) => (
                      <SelectItem key={key} value={`input:${key}`}>
                        Snippet input: {key}
                      </SelectItem>
                    ))}
                    {sources.map((source) => (
                      <SelectItem
                        key={source.value}
                        value={`output:${source.value}`}
                      >
                        {source.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {outputReference ? (
                  <p className="text-xs text-aurora-text-muted">
                    Uses {outputReference}. Resolved after its dependency
                    succeeds.
                  </p>
                ) : reference ? (
                  <p className="text-xs text-aurora-text-muted">
                    Uses input.{reference}
                  </p>
                ) : property.enum ? (
                  <Select
                    value={
                      current === undefined
                        ? '__unset'
                        : JSON.stringify(current)
                    }
                    onValueChange={(next) =>
                      change(
                        name,
                        next === '__unset' ? undefined : JSON.parse(next),
                      )
                    }
                  >
                    <SelectTrigger id={id}>
                      <SelectValue placeholder="Choose a value" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__unset">Not set</SelectItem>
                      {property.enum.map((item, i) => (
                        <SelectItem key={i} value={JSON.stringify(item)}>
                          {String(item)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                ) : property.type === 'boolean' ? (
                  <Select
                    value={current === undefined ? 'unset' : String(current)}
                    onValueChange={(next) =>
                      change(
                        name,
                        next === 'unset' ? undefined : next === 'true',
                      )
                    }
                  >
                    <SelectTrigger id={id}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="unset">Not set</SelectItem>
                      <SelectItem value="true">True</SelectItem>
                      <SelectItem value="false">False</SelectItem>
                    </SelectContent>
                  </Select>
                ) : ['object', 'array'].includes(String(property.type)) ? (
                  <Textarea
                    id={id}
                    value={
                      current === undefined
                        ? ''
                        : typeof current === 'string'
                          ? current
                          : JSON.stringify(current)
                    }
                    placeholder={property.type === 'array' ? '[]' : '{}'}
                    onChange={(event) => {
                      try {
                        const next = event.target.value
                        change(name, next.trim() ? JSON.parse(next) : undefined)
                        setError(undefined)
                      } catch {
                        change(name, event.target.value)
                        setError(`${name} must be valid JSON.`)
                      }
                    }}
                    className="font-mono text-xs"
                  />
                ) : (
                  <Input
                    id={id}
                    type={
                      ['number', 'integer'].includes(String(property.type))
                        ? 'number'
                        : 'text'
                    }
                    step={property.type === 'integer' ? 1 : 'any'}
                    min={property.minimum}
                    max={property.maximum}
                    value={current === undefined ? '' : String(current)}
                    onChange={(event) => {
                      const raw = event.target.value
                      change(
                        name,
                        raw === ''
                          ? undefined
                          : ['number', 'integer'].includes(
                                String(property.type),
                              )
                            ? Number(raw)
                            : raw,
                      )
                    }}
                  />
                )}
                {current === undefined &&
                schema.required?.includes(name) &&
                onAddInput ? (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => onAddInput(name, property)}
                  >
                    Declare input for {name}
                  </Button>
                ) : null}
                {property.description ? (
                  <p className="text-xs text-aurora-text-muted">
                    {property.description}
                  </p>
                ) : null}
              </div>
            )
          })}
        </div>
      ) : (
        <>
          <Textarea
            id={`tool-mapping-${index}`}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className="min-h-20 font-mono text-xs"
          />
          <p className="text-xs text-aurora-text-muted">
            Use JSON constants or "$input.name" to map a declared snippet input.
          </p>
        </>
      )}
    </div>
  )
}
