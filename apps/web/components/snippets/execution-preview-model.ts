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
  schema?: ParameterSchema,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(inputs)
      .filter(
        ([key, spec]) =>
          spec.default !== undefined &&
          !containsSensitiveValue(spec.default, schema?.properties?.[key], key),
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
  // Union sensitivity from every target: a safe use cannot unmask another use.
  const merge = (target: ParameterSchema, source: ParameterSchema) => {
    if (sensitiveWorkflowSchema(source)) target.writeOnly = true
    for (const [key, child] of Object.entries(source.properties ?? {})) {
      target.properties ??= Object.create(null)
      if (!Object.prototype.hasOwnProperty.call(target.properties, key))
        target.properties![key] = {}
      merge(target.properties![key], child)
    }
    if (source.items) {
      target.items ??= {}
      merge(target.items, source.items)
    }
  }
  const sensitivity = (schema?: ParameterSchema, name = ''): ParameterSchema => {
    if (!schema || (['$ref', 'allOf', 'anyOf', 'oneOf', 'patternProperties'].some(key => key in schema) || ('additionalProperties' in schema && schema.additionalProperties !== false)) || sensitiveWorkflowSchema(schema) || sensitiveWorkflowName(name))
      return { writeOnly: true }
    return {
      properties: Object.fromEntries(Object.entries(schema?.properties ?? {})
        .map(([key, child]) => [key, sensitivity(child, key)])),
      ...(schema?.items ? { items: sensitivity(schema.items) } : {}),
    }
  }
  const mark = (path: string[], schema: ParameterSchema) => {
    let current = root
    for (const [index, key] of path.entries()) {
      if (!key || ['__proto__', 'constructor', 'prototype'].includes(key)) return
      // The first segment always names an input, including a numeric name.
      // Numeric descendants may be array elements: mask their containing input.
      if (index > 0 && /^\d+$/.test(key)) { current.writeOnly = true; return }
      current.properties ??= Object.create(null)
      if (!Object.prototype.hasOwnProperty.call(current.properties, key))
        current.properties![key] = { properties: Object.create(null) }
      current = current.properties![key]
    }
    merge(current, schema)
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
    if (typeof value === 'string' && value.startsWith('$input.'))
      mark(value.slice(7).split('.'), sensitive ? { writeOnly: true } : sensitivity(schema))
    else if (Array.isArray(value))
      value.forEach((item) => walk(item, schema?.items, '', sensitive))
    else if (value && typeof value === 'object')
      for (const [key, item] of Object.entries(value))
        walk(item, schema?.properties?.[key], key, sensitive)
  }
  for (const step of plan?.steps ?? []) walk(step.mapping, schemas[step.id])
  return root
}
