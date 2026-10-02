import type { ParameterSchema } from './tool-parameter-model'
import { safeSegment } from './workflow-selectors'
import { sensitiveWorkflowName, sensitiveWorkflowSchema } from './workflow-redaction'
export interface WorkflowMappingSource { id: string; outputSchema?: ParameterSchema }
export interface WorkflowInputSchema { [name: string]: ParameterSchema }

function compatible(target: ParameterSchema, source: ParameterSchema): boolean {
  if (['$ref', 'oneOf', 'anyOf', 'allOf'].some((key) => key in target || key in source)) return false
  if (typeof target.type !== 'string' || typeof source.type !== 'string') return false
  if (!['string', 'boolean', 'number', 'integer'].includes(target.type)) return false
  if (target.type !== source.type && !(target.type === 'number' && source.type === 'integer')) return false
  if (target.enum && (!source.enum || !source.enum.every((entry) => target.enum!.some((allowed) => JSON.stringify(allowed) === JSON.stringify(entry))))) return false
  for (const [lower, upper] of [['minimum', 'maximum'], ['minLength', 'maxLength']]) {
    if (typeof target[lower] === 'number' && (typeof source[lower] !== 'number' || (source[lower] as number) < (target[lower] as number))) return false
    if (typeof target[upper] === 'number' && (typeof source[upper] !== 'number' || (source[upper] as number) > (target[upper] as number))) return false
  }
  return true
}
/** Same-name, compatible scalar fields only; ambiguity is deliberately left for the user. */
export function suggestWorkflowMapping(target: ParameterSchema, inputs: WorkflowInputSchema, previous: WorkflowMappingSource[] = []): Record<string, unknown> {
  const candidates: { name: string; schema: ParameterSchema; expression: string }[] = []
  for (const [name, schema] of Object.entries(inputs)) if (safeSegment(name) && !sensitiveWorkflowName(name) && !sensitiveWorkflowSchema(schema) && !name.includes('.')) candidates.push({ name, schema, expression: `$input.${name}` })
  const collect = (id: string, schema: ParameterSchema, path: string[], depth: number) => {
    if (depth > 8 || schema.type !== 'object') return
    for (const [name, child] of Object.entries(schema.properties ?? {})) {
      if (!safeSegment(name) || sensitiveWorkflowName(name) || sensitiveWorkflowSchema(child) || name.includes('.')) continue
      const next = [...path, name]
      candidates.push({ name, schema: child, expression: `$steps.${id}.${next.join('.')}` })
      collect(id, child, next, depth + 1)
    }
  }
  for (const step of previous) if (safeSegment(step.id) && !step.id.includes('.') && step.outputSchema) collect(step.id, step.outputSchema, [], 0)
  return Object.fromEntries(Object.entries(target.properties ?? {}).flatMap(([name, schema]) => {
    if (!safeSegment(name) || sensitiveWorkflowName(name) || sensitiveWorkflowSchema(schema)) return []
    const matches = candidates.filter((candidate) => candidate.name === name && compatible(schema, candidate.schema))
    return matches.length === 1 ? [[name, matches[0].expression]] : []
  }))
}
