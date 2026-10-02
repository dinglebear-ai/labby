import type { ParameterSchema } from './tool-parameter-model'
export const WORKFLOW_REDACTED = '[redacted]'
export function sensitiveWorkflowName(name: string): boolean {
  return /(?:secret|token|password|passwd|credential|api[_-]?key|authorization|private[_-]?key)/i.test(name)
}
export function sensitiveWorkflowSchema(schema?: ParameterSchema): boolean {
  return schema?.writeOnly === true || schema?.format === 'password'
}
export function redactWorkflowValue(value: unknown, schema?: ParameterSchema, name = ''): unknown {
  if (sensitiveWorkflowName(name) || sensitiveWorkflowSchema(schema)) return WORKFLOW_REDACTED
  if (Array.isArray(value)) return value.map((entry) => redactWorkflowValue(entry, schema?.items))
  if (typeof value === 'object' && value !== null) return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, redactWorkflowValue(entry, schema?.properties?.[key], key)]))
  return value
}
