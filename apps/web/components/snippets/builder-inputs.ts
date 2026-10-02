import type { SnippetInputSpec, SnippetInputType } from '@/lib/types/snippets'
import type { ParameterSchema } from './tool-parameter-model'
import { containsSensitiveValue } from './execution-preview-model'
export function inputSpecFromSchema(schema: ParameterSchema): SnippetInputSpec {
  const supported = [
    'string',
    'integer',
    'number',
    'boolean',
    'object',
    'array',
  ]
  const ty =
    typeof schema.type === 'string' && supported.includes(schema.type)
      ? (schema.type as SnippetInputType)
      : 'json'
  return {
    ty,
    required: true,
    ...(typeof schema.description === 'string'
      ? { description: schema.description }
      : {}),
  }
}
export function builderInputSpecs(
  existing: Record<string, SnippetInputSpec>,
  examples: Record<string, unknown>,
): Record<string, SnippetInputSpec> {
  const result = { ...existing }
  for (const [name, value] of Object.entries(examples))
    if (!Object.prototype.hasOwnProperty.call(result, name)) {
      const ty =
        typeof value === 'boolean'
          ? 'boolean'
          : typeof value === 'number'
            ? Number.isInteger(value)
              ? 'integer'
              : 'number'
            : typeof value === 'string'
              ? 'string'
              : 'json'
      result[name] = {
        ty,
        ...(containsSensitiveValue(value, undefined, name)
          ? { required: true }
          : { default: value, required: false }),
        ...(value === null ? { nullable: true } : {}),
      }
    }
  return result
}
export function inputSpecsFrontmatter(
  specs: Record<string, SnippetInputSpec>,
): string[] {
  return Object.entries(specs).flatMap(([name, spec]) => {
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(name))
      throw new Error(`Invalid declared input name ${name}`)
    if (
      spec.description &&
      (/[\r\n]/.test(spec.description) ||
        spec.description.startsWith('"') ||
        spec.description.endsWith('"'))
    )
      throw new Error(
        `Input ${name} description must fit on one line without boundary quotes.`,
      )
    return [
      `  ${name}:`,
      `    type: ${spec.ty}`,
      ...(spec.required !== undefined
        ? [`    required: ${spec.required}`]
        : []),
      ...(spec.nullable !== undefined
        ? [`    nullable: ${spec.nullable}`]
        : []),
      ...(Object.prototype.hasOwnProperty.call(spec, 'default')
        ? [`    default: ${JSON.stringify(spec.default)}`]
        : []),
      ...(spec.description ? [`    description: ${spec.description}`] : []),
    ]
  })
}
