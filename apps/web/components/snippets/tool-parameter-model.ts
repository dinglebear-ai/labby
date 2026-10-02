export interface ParameterSchema {
  type?: string | string[]
  title?: string
  description?: string
  enum?: unknown[]
  properties?: Record<string, ParameterSchema>
  required?: string[]
  items?: ParameterSchema
  minimum?: number
  maximum?: number
  minLength?: number
  maxLength?: number
  additionalProperties?: boolean | ParameterSchema
  [key: string]: unknown
}

export function objectSchema(value: unknown): ParameterSchema | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return undefined
  const schema = value as ParameterSchema
  if (schema.type !== 'object' || typeof schema.properties !== 'object' || schema.properties === null || Array.isArray(schema.properties)) return undefined
  if (schema.required !== undefined && (!Array.isArray(schema.required) || !schema.required.every((key) => typeof key === 'string'))) return undefined
  if (!Object.values(schema.properties).every((property) => typeof property === 'object' && property !== null && !Array.isArray(property) && (property.enum === undefined || Array.isArray(property.enum)))) return undefined
  return schema
}

export function guidedSchemaSupported(schema: ParameterSchema): boolean {
  if (['$ref', 'oneOf', 'anyOf', 'allOf'].some((key) => key in schema)) return false
  return Object.values(schema.properties ?? {}).every((property) => property && typeof property === 'object' &&
    !['$ref', 'oneOf', 'anyOf', 'allOf'].some((key) => key in property) &&
    typeof property.type === 'string' && ['string', 'integer', 'number', 'boolean', 'object', 'array'].includes(property.type))
}

function valueError(schema: ParameterSchema, value: unknown): string | undefined {
  const types = Array.isArray(schema.type) ? schema.type : [schema.type]
  const matches = types.some((type) => type === undefined ||
    (type === 'null' && value === null) || (type === 'array' && Array.isArray(value)) ||
    (type === 'object' && typeof value === 'object' && value !== null && !Array.isArray(value)) ||
    (type === 'integer' && typeof value === 'number' && Number.isInteger(value)) ||
    (type === 'number' && typeof value === 'number' && Number.isFinite(value)) ||
    (['string', 'boolean'].includes(type!) && typeof value === type))
  if (!matches) return `must be ${types.join(' or ')}`
  if (schema.enum && !schema.enum.some((entry) => JSON.stringify(entry) === JSON.stringify(value))) return 'must be one of the allowed values'
  if (typeof value === 'number') {
    if (typeof schema.minimum === 'number' && value < schema.minimum) return `must be at least ${schema.minimum}`
    if (typeof schema.maximum === 'number' && value > schema.maximum) return `must be at most ${schema.maximum}`
  }
  if (typeof value === 'string') {
    if (typeof schema.minLength === 'number' && value.length < schema.minLength) return `must contain at least ${schema.minLength} characters`
    if (typeof schema.maxLength === 'number' && value.length > schema.maxLength) return `must contain at most ${schema.maxLength} characters`
  }
  return undefined
}

/** Validate supported top-level constraints; backend remains authoritative. */
export function mappedParameterError(schema: ParameterSchema, mapping: Record<string, unknown>, inputs: Record<string, unknown>): string | undefined {
  for (const required of schema.required ?? []) {
    if (!Object.prototype.hasOwnProperty.call(mapping, required)) return `Required parameter "${required}" is missing.`
  }
  for (const [name, value] of Object.entries(mapping)) {
    const property = schema.properties && Object.prototype.hasOwnProperty.call(schema.properties, name) ? schema.properties[name] : undefined
    if (!property) {
      if (schema.additionalProperties === false) return `Unknown parameter "${name}".`
      continue
    }
    if (typeof value === 'string' && value.startsWith('$input.') && !Object.prototype.hasOwnProperty.call(inputs, value.slice(7))) return `Unknown snippet input "${value.slice(7)}".`
    const resolved = typeof value === 'string' && value.startsWith('$input.') ? inputs[value.slice(7)] : value
    const error = valueError(property, resolved)
    if (error) return `Parameter "${name}" ${error}.`
  }
  return undefined
}
