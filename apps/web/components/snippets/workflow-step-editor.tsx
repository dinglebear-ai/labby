'use client'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  searchCodeModeTools,
  describeCodeModeTool,
  type ToolDescription,
  type ToolSearchHit,
} from '@/lib/api/tool-browser-client'
import { ToolParameterForm } from './tool-parameter-form'
import { containsSensitiveValue } from './execution-preview-model'
import { objectSchema, type ParameterSchema } from './tool-parameter-model'
import {
  previewWorkflow,
  renameWorkflowStep,
  suggestWorkflowMapping,
  type WorkflowPlan,
} from './workflow-model'

export function WorkflowStepEditor({
  plan,
  onChange,
  inputs,
  declaredInputs = {},
  onInputsChange,
  schemas,
  onSchema,
  onValidationError,
  onInputSchema,
  sequential = false,
}: {
  plan: WorkflowPlan
  onChange: (plan: WorkflowPlan) => void
  declaredInputs?: Record<
    string,
    import('@/lib/types/snippets').SnippetInputSpec
  >
  inputs: Record<string, unknown>
  onInputsChange: (inputs: Record<string, unknown>) => void
  schemas: Record<string, ParameterSchema>
  onValidationError: (error: string) => void
  sequential?: boolean
  onInputSchema: (name: string, schema: ParameterSchema) => void
  onSchema: (id: string, schema: ParameterSchema) => void
}) {
  const [nameDrafts, setNameDrafts] = useState<Record<string, string>>({})
  const [renameError, setRenameError] = useState('')
  const [query, setQuery] = useState('')
  const [hits, setHits] = useState<ToolSearchHit[]>([])
  const [error, setError] = useState('')
  const [descriptions, setDescriptions] = useState<
    Record<string, ToolDescription>
  >({})
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const cache = useRef(new Map<string, Promise<ToolDescription>>())
  const loader = useCallback((tool: string) => {
    let pending = cache.current.get(tool)
    if (!pending) {
      pending = describeCodeModeTool(tool)
      cache.current.set(tool, pending)
      pending.catch(() => cache.current.delete(tool))
    }
    return pending
  }, [])
  useEffect(() => {
    if (!query.trim()) {
      setHits([])
      return
    }
    const controller = new AbortController()
    const timer = setTimeout(() => {
      void searchCodeModeTools(query, controller.signal)
        .then((result) => {
          if (!controller.signal.aborted) {
            setHits(result.results)
            setError('')
          }
        })
        .catch((e) => {
          if (!controller.signal.aborted) setError(String(e))
        })
    }, 250)
    return () => {
      clearTimeout(timer)
      controller.abort()
    }
  }, [query])
  useEffect(() => {
    const invalid = Object.entries(drafts).find(
      ([id, value]) =>
        plan.steps.some((step) => step.id === id) &&
        (() => {
          try {
            const parsed = JSON.parse(value)
            return (
              !parsed || typeof parsed !== 'object' || Array.isArray(parsed)
            )
          } catch {
            return true
          }
        })(),
    )
    onValidationError(
      renameError ||
        (invalid ? `${invalid[0]}: repair parameter JSON before building` : ''),
    )
  }, [drafts, plan, onValidationError, renameError])
  const patch = (id: string, changes: Partial<WorkflowPlan['steps'][number]>) =>
    onChange({
      ...plan,
      steps: plan.steps.map((step) =>
        step.id === id ? { ...step, ...changes } : step,
      ),
    })
  const rename = (oldId: string) => {
    const newId = (nameDrafts[oldId] ?? oldId).trim()
    if (newId === oldId) return
    try {
      const next = renameWorkflowStep(plan, oldId, newId)
      if (schemas[oldId]) onSchema(newId, schemas[oldId])
      setDescriptions((current) => {
        const value = { ...current }
        if (value[oldId]) {
          value[newId] = value[oldId]
          delete value[oldId]
        }
        return value
      })
      setDrafts((current) =>
        Object.fromEntries(
          Object.entries(current).map(([id, value]) => {
            const target = id === oldId ? newId : id
            try {
              JSON.parse(value)
              return [
                target,
                JSON.stringify(
                  next.steps.find((step) => step.id === target)?.mapping ?? {},
                  null,
                  2,
                ),
              ]
            } catch {
              return [target, value]
            }
          }),
        ),
      )
      setNameDrafts((current) => {
        const value = { ...current }
        delete value[oldId]
        return value
      })
      setRenameError('')
      onChange(next)
    } catch (error) {
      setRenameError(
        error instanceof Error ? error.message : 'Invalid step name',
      )
    }
  }
  const preview = previewWorkflow(plan, inputs, schemas)
  const inputSchemas = {
    ...Object.fromEntries(
      Object.entries(declaredInputs).map(([name, spec]) => [
        name,
        { type: spec.ty },
      ]),
    ),
    ...Object.fromEntries(
      Object.entries(inputs).map(([name, value]) => [
        name,
        {
          type: Array.isArray(value)
            ? 'array'
            : value === null
              ? 'null'
              : typeof value === 'object'
                ? 'object'
                : typeof value,
        },
      ]),
    ),
  }
  return (
    <div className="grid gap-3">
      <Label htmlFor="workflow-tool-search">Find a tool</Label>
      <Input
        id="workflow-tool-search"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder="Search the authorized tool catalog"
      />
      {error ? <p role="alert">{error}</p> : null}
      {renameError ? <p role="alert">{renameError}</p> : null}
      {hits.map((hit) => (
        <Button
          key={hit.id}
          variant="outline"
          type="button"
          className="h-auto justify-start whitespace-normal text-left"
          onClick={() => {
            let number = 1
            while (plan.steps.some((step) => step.id === `step-${number}`))
              number++
            onChange({
              ...plan,
              steps: [
                ...plan.steps,
                {
                  id: `step-${number}`,
                  tool: hit.id,
                  mapping: {},
                  dependsOn:
                    sequential && plan.steps.length
                      ? [plan.steps[plan.steps.length - 1].id]
                      : [],
                },
              ],
            })
            setQuery('')
          }}
        >
          Add {hit.id} · {hit.description}
        </Button>
      ))}
      {plan.steps.map((step, index) => {
        const previous = plan.steps.slice(0, index)
        const sources = previous.flatMap((prior) => {
          const output = objectSchema(descriptions[prior.id]?.output_schema)
          return [
            {
              value: `$steps.${prior.id}`,
              label: `${prior.id}: entire output`,
            },
            ...Object.keys(output?.properties ?? {}).map((name) => ({
              value: `$steps.${prior.id}.${name}`,
              label: `${prior.id} output: ${name}`,
            })),
          ]
        })
        const suggested = schemas[step.id]
          ? {
              ...Object.fromEntries(
                Object.entries(schemas[step.id].properties ?? {})
                  .filter(
                    ([name, schema]) =>
                      schema.default !== undefined &&
                      !containsSensitiveValue(schema.default, schema, name),
                  )
                  .map(([name, schema]) => [name, schema.default]),
              ),
              ...suggestWorkflowMapping(
                schemas[step.id],
                inputSchemas,
                previous.map((prior) => ({
                  id: prior.id,
                  outputSchema: objectSchema(
                    descriptions[prior.id]?.output_schema,
                  ),
                })),
              ),
            }
          : {}
        return (
          <section
            key={step.id}
            className="grid gap-2 rounded-aurora-2 border border-aurora-border-subtle p-3"
          >
            <strong>
              {step.id} · {step.tool}
            </strong>
            <Label htmlFor={`step-name-${index}`}>Step name</Label>
            <Input
              id={`step-name-${index}`}
              value={nameDrafts[step.id] ?? step.id}
              onChange={(event) =>
                setNameDrafts((current) => ({
                  ...current,
                  [step.id]: event.target.value,
                }))
              }
              onBlur={() => rename(step.id)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault()
                  rename(step.id)
                }
              }}
            />
            <div className="flex gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={index === 0}
                onClick={() => {
                  const steps = [...plan.steps]
                  ;[steps[index - 1], steps[index]] = [
                    steps[index],
                    steps[index - 1],
                  ]
                  onChange({ ...plan, steps })
                }}
              >
                Move up
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={index === plan.steps.length - 1}
                onClick={() => {
                  const steps = [...plan.steps]
                  ;[steps[index + 1], steps[index]] = [
                    steps[index],
                    steps[index + 1],
                  ]
                  onChange({ ...plan, steps })
                }}
              >
                Move down
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() =>
                  onChange({
                    ...plan,
                    steps: plan.steps.filter((item) => item.id !== step.id),
                  })
                }
              >
                Delete step
              </Button>
            </div>
            <Label htmlFor={`dependencies-${step.id}`}>
              Named dependencies (comma separated)
            </Label>
            <Input
              id={`dependencies-${step.id}`}
              value={step.dependsOn.join(', ')}
              onChange={(event) =>
                patch(step.id, {
                  dependsOn: event.target.value
                    .split(',')
                    .map((value) => value.trim())
                    .filter(Boolean),
                })
              }
            />
            <div className="flex flex-wrap gap-2">
              {plan.steps
                .filter((prior) => prior.id !== step.id)
                .map((prior) => (
                  <label
                    key={prior.id}
                    className="flex items-center gap-1 text-xs"
                  >
                    <input
                      type="checkbox"
                      checked={step.dependsOn.includes(prior.id)}
                      onChange={(event) =>
                        patch(step.id, {
                          dependsOn: event.target.checked
                            ? [...step.dependsOn, prior.id]
                            : step.dependsOn.filter((id) => id !== prior.id),
                        })
                      }
                    />
                    After {prior.id}
                  </label>
                ))}
            </div>
            <ToolParameterForm
              tool={step.tool}
              index={index}
              value={drafts[step.id] ?? JSON.stringify(step.mapping, null, 2)}
              inputs={inputs}
              declaredInputs={declaredInputs}
              autoLoad
              descriptionLoader={loader}
              onRefresh={() => cache.current.delete(step.tool)}
              sources={sources}
              suggestions={suggested}
              onDescription={(description) =>
                setDescriptions((current) => ({
                  ...current,
                  [step.id]: description,
                }))
              }
              onSchema={(schema) => onSchema(step.id, schema)}
              onChange={(value) => {
                setDrafts((current) => ({ ...current, [step.id]: value }))
                try {
                  const mapping = JSON.parse(value)
                  if (
                    mapping &&
                    typeof mapping === 'object' &&
                    !Array.isArray(mapping)
                  ) {
                    patch(step.id, { mapping })
                    setError('')
                  } else setError(`${step.id}: parameters must be an object`)
                } catch {
                  setError(`${step.id}: repair invalid JSON before building`)
                }
              }}
              onAddInput={(name, schema) => {
                onInputSchema(name, schema)
                if (
                  schema.default !== undefined &&
                  !containsSensitiveValue(schema.default, schema, name) &&
                  !Object.prototype.hasOwnProperty.call(inputs, name)
                )
                  onInputsChange(
                    Object.fromEntries([
                      ...Object.entries(inputs),
                      [name, schema.default],
                    ]),
                  )
                patch(step.id, {
                  mapping: { ...step.mapping, [name]: `$input.${name}` },
                })
                setDrafts((current) => {
                  const next = { ...current }
                  delete next[step.id]
                  return next
                })
              }}
            />
            <p className="text-xs text-aurora-text-muted">
              {descriptions[step.id]?.output_schema
                ? 'Known output fields are available in later steps.'
                : 'Output schema unknown: use an explicit $steps.stepId.path selector in Advanced JSON.'}
            </p>
          </section>
        )
      })}
      <div className="text-xs">
        <strong>Execution waves</strong>
        {preview.waves.map((wave, index) => (
          <p key={index}>
            Wave {index + 1}: {wave.join(', ')}
          </p>
        ))}
        <p>
          Independent calls preserve individual statuses. Failed dependencies
          skip their dependent steps.
        </p>
        {preview.errors.map((message) => (
          <p key={message} role="alert" className="text-aurora-error">
            {message}
          </p>
        ))}
        <pre className="overflow-auto">
          {JSON.stringify(
            preview.steps.map((step) => ({
              ...step,
              params: schemas[step.id]
                ? step.params
                : {
                    keys: Object.keys(step.params),
                    values: 'Tool schema unavailable: sensitivity unresolved',
                  },
            })),
            null,
            2,
          )}
        </pre>
      </div>
    </div>
  )
}
