import type { SnippetInputSpec } from '@/lib/types/snippets'
import type { ParameterSchema } from './tool-parameter-model'
import type { WorkflowPlan } from './workflow-model'
import {
  sensitiveWorkflowName,
  sensitiveWorkflowSchema,
} from './workflow-redaction'

export function containsSensitiveValue(
  value: unknown,
  schema?: ParameterSchema,
  name = '',
): boolean {
  if (sensitiveWorkflowName(name) || sensitiveWorkflowSchema(schema))
    return true
  if (Array.isArray(value))
    return value.some((item) => containsSensitiveValue(item, schema?.items))
  if (value && typeof value === 'object')
    return Object.entries(value).some(([key, item]) =>
      containsSensitiveValue(item, schema?.properties?.[key], key),
    )
  return false
}
export function safeReplayDefaults(
  inputs: Record<string, SnippetInputSpec>,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(inputs)
      .filter(
        ([key, spec]) =>
          spec.default !== undefined &&
          !containsSensitiveValue(spec.default, undefined, key),
      )
      .map(([key, spec]) => [key, spec.default]),
  )
}
export function inheritedInputSchema(
  plan: WorkflowPlan | undefined,
  schemas: Record<string, ParameterSchema>,
): ParameterSchema {
  const root: ParameterSchema = {
    type: 'object',
    properties: Object.create(null),
  }
  const mark = (path: string[]) => {
    let current = root
    for (const key of path) {
      if (!key || ['__proto__', 'constructor', 'prototype'].includes(key))
        return
      current.properties ??= Object.create(null)
      current.properties![key] ??= {
        type: 'object',
        properties: Object.create(null),
      }
      current = current.properties![key]
    }
    current.writeOnly = true
  }
  const walk = (
    value: unknown,
    schema?: ParameterSchema,
    name = '',
    inherited = false,
  ) => {
    const sensitive =
      inherited ||
      sensitiveWorkflowSchema(schema) ||
      sensitiveWorkflowName(name)
    if (typeof value === 'string' && value.startsWith('$input.') && sensitive)
      mark(value.slice(7).split('.'))
    else if (Array.isArray(value))
      value.forEach((item) => walk(item, schema?.items, '', sensitive))
    else if (value && typeof value === 'object')
      for (const [key, item] of Object.entries(value))
        walk(item, schema?.properties?.[key], key, sensitive)
  }
  for (const step of plan?.steps ?? []) walk(step.mapping, schemas[step.id])
  return root
}
