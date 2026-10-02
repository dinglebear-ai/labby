'use client'
import { useEffect, useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Label } from '@/components/ui/label'
import { snippetsApi } from '@/lib/api/snippets-client'
import type { ResolvedSnippet, SnippetPreview } from '@/lib/types/snippets'
import { previewWorkflow, readWorkflowPlan } from './workflow-model'
import { redactWorkflowValue } from './workflow-redaction'
import {
  safeReplayDefaults,
  inheritedInputSchema,
} from './execution-preview-model'
import { describeCodeModeTool } from '@/lib/api/tool-browser-client'
import { objectSchema, type ParameterSchema } from './tool-parameter-model'

export function ExecutionPreview({
  name,
  executionId,
  initialParams,
  onClose,
  onRun,
}: {
  name: string
  executionId?: string
  initialParams: Record<string, unknown>
  onClose: () => void
  onRun: (run: () => Promise<unknown>) => void
}) {
  const [snippet, setSnippet] = useState<ResolvedSnippet>()
  const [params, setParams] = useState(JSON.stringify(initialParams, null, 2))
  const [preview, setPreview] = useState<SnippetPreview>()
  const [error, setError] = useState('')
  const [ack, setAck] = useState(false)
  const [loading, setLoading] = useState(true)
  const [schemas, setSchemas] = useState<Record<string, ParameterSchema>>({})
  const [schemaReady, setSchemaReady] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    void snippetsApi
      .get(name, controller.signal)
      .then(async (detail) => {
        if (controller.signal.aborted) return
        setSnippet(detail)
        if (executionId)
          setParams(
            JSON.stringify(safeReplayDefaults(detail.inputs ?? {}), null, 2),
          )
        const plan = readWorkflowPlan(detail.body)
        const tools = [...new Set(plan?.steps.map((step) => step.tool) ?? [])]
        const loaded: Record<string, ParameterSchema> = {}
        // Bounded serial descriptions coalesce duplicate step tools.
        for (const tool of tools) {
          if (controller.signal.aborted) return
          try {
            const description = await describeCodeModeTool(
              tool,
              controller.signal,
            )
            const schema = objectSchema(description.input_schema)
            if (schema)
              for (const step of plan?.steps ?? [])
                if (step.tool === tool) loaded[step.id] = schema
          } catch {
            /* Missing schemas remain unknown. */
          }
        }
        if (!controller.signal.aborted) {
          setSchemas(loaded)
          setSchemaReady(true)
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(String(e))
      })
    return () => controller.abort()
  }, [name, executionId])
  useEffect(() => {
    if (!snippet) return
    const controller = new AbortController()
    setPreview(undefined)
    setAck(false)
    setLoading(true)
    const timer = setTimeout(() => {
      let parsed: unknown
      try {
        parsed = JSON.parse(params)
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed))
          throw new Error('Inputs must be a JSON object')
      } catch (e) {
        setError(String(e))
        setLoading(false)
        return
      }
      void snippetsApi
        .preview(
          {
            ...(executionId ? { execution_id: executionId } : { name }),
            params: parsed as Record<string, unknown>,
          },
          controller.signal,
        )
        .then((value) => {
          if (!controller.signal.aborted) {
            setPreview(value)
            setError('')
          }
        })
        .catch((e) => {
          if (!controller.signal.aborted) setError(String(e))
        })
        .finally(() => {
          if (!controller.signal.aborted) setLoading(false)
        })
    }, 200)
    return () => {
      clearTimeout(timer)
      controller.abort()
    }
  }, [name, executionId, params, snippet])
  let input: Record<string, unknown> = {}
  try {
    const parsed: unknown = JSON.parse(params)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed))
      input = parsed as Record<string, unknown>
  } catch {
    /* Keep invalid JSON editable. */
  }
  const merged = Object.fromEntries([
    ...Object.entries(snippet?.inputs ?? {})
      .filter(([, spec]) => spec.default !== undefined)
      .map(([key, spec]) => [key, spec.default]),
    ...Object.entries(input),
  ])
  const plan = snippet ? readWorkflowPlan(snippet.body) : undefined
  const workflow = plan ? previewWorkflow(plan, merged, schemas) : undefined
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>
            {executionId ? 'Replay' : 'Execution'} preview · {name}
          </DialogTitle>
          <DialogDescription>
            Review current inputs, authority, and tool permissions. Run now
            executes the current snippet. Receipts never retain raw inputs.
            {executionId
              ? ' Starts a new run of the whole workflow; it does not resume the earlier run.'
              : null}
          </DialogDescription>
        </DialogHeader>
        {executionId &&
        preview &&
        (preview.coverage === 'unrestricted_dynamic' ||
          preview.declared_tools.some(
            (tool) => tool.annotations?.read_only !== true,
          )) ? (
          <p className="text-xs">
            Previous successful writes may repeat because current tool behavior
            may write or is unknown.
          </p>
        ) : null}
        {snippet &&
        Object.entries(snippet.inputs ?? {}).some(
          ([, spec]) => spec.required,
        ) ? (
          <p className="text-xs">
            Required inputs:{' '}
            {Object.entries(snippet.inputs ?? {})
              .filter(([, spec]) => spec.required)
              .map(
                ([name, spec]) =>
                  `${name} (${spec.ty}${spec.nullable ? ', nullable' : ''})`,
              )
              .join(', ')}
            . Enter fresh values before Run now.
          </p>
        ) : null}
        <Label htmlFor="preview-inputs">Current run inputs (JSON)</Label>
        <Textarea
          id="preview-inputs"
          value={params}
          onChange={(event) => setParams(event.target.value)}
          className="font-mono text-xs"
        />
        <p className="text-xs">Redacted input preview</p>
        <pre className="overflow-auto text-xs">
          {JSON.stringify(
            plan && schemaReady && plan.steps.every((step) => schemas[step.id])
              ? redactWorkflowValue(merged, inheritedInputSchema(plan, schemas))
              : {
                  keys: Object.keys(merged),
                  values:
                    'Unresolved: input sensitivity requires known target schemas',
                },
            null,
            2,
          )}
        </pre>
        {workflow && schemaReady ? (
          <>
            <strong>Known builder parameter plan</strong>
            {workflow.steps.map((step) => (
              <div key={step.id} className="text-xs">
                <p>
                  {step.id} · {step.tool} · wave {step.wave + 1}
                </p>
                <pre className="overflow-auto">
                  {JSON.stringify(
                    schemas[step.id]
                      ? step.params
                      : {
                          keys: Object.keys(step.params),
                          values:
                            'Unresolved sensitivity: tool schema unavailable',
                        },
                    null,
                    2,
                  )}
                </pre>
                {step.unresolved.map((reference) => (
                  <p key={reference}>Unresolved until execution: {reference}</p>
                ))}
              </div>
            ))}
          </>
        ) : (
          <p className="text-xs">
            Dynamic JavaScript: actual call parameters and order are unresolved
            until execution. Declared tools do not prove which calls the script
            makes.
          </p>
        )}
        {loading ? (
          <p role="status">Checking current execution authority…</p>
        ) : null}
        {error ? (
          <p role="alert" className="text-aurora-error">
            {error}
          </p>
        ) : null}
        {preview ? (
          <>
            <div className="text-xs">
              {preview.declared_tools.map((tool) => (
                <p key={tool.id}>
                  {tool.id} · {tool.status} · parameters {tool.parameters} ·
                  safety{' '}
                  {tool.annotations?.destructive === true
                    ? 'destructive'
                    : tool.annotations?.read_only === true
                      ? 'read only'
                      : tool.annotations?.read_only === false
                        ? 'may write'
                        : 'unknown'}
                </p>
              ))}
            </div>
            {preview.warnings.map((warning) => (
              <p key={warning} className="text-xs">
                {warning}
              </p>
            ))}
            {preview.drift.length ? (
              <label className="flex gap-2 text-xs">
                <input
                  type="checkbox"
                  checked={ack}
                  onChange={(event) => setAck(event.target.checked)}
                />
                <span>
                  I acknowledge current drift:{' '}
                  {preview.drift
                    .map((item) => `${item.field} (${item.status})`)
                    .join(', ')}
                </span>
              </label>
            ) : null}
          </>
        ) : null}
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={
              !preview?.can_execute ||
              loading ||
              !!error ||
              (preview.drift.length > 0 && !ack)
            }
            onClick={() => {
              if (!preview) return
              const values = JSON.parse(params) as Record<string, unknown>
              const fingerprint = preview.preview_fingerprint
              const drift = preview.drift.map((item) => item.field)
              onRun(() =>
                executionId
                  ? snippetsApi.replay(executionId, values, fingerprint, drift)
                  : snippetsApi.exec(name, values, undefined, fingerprint),
              )
              onClose()
            }}
          >
            Run now
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
