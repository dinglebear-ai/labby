export type OperationBaseType = 'string' | 'boolean' | 'integer' | 'number' | 'array' | 'object'
export type OperationSchemaType = OperationBaseType | [OperationBaseType, 'null'] | ['null', OperationBaseType]

export type OperationBaseProperty = {
  type?: OperationSchemaType
  description?: string
  enum?: Array<string | number>
  minimum?: number
  maximum?: number
  minLength?: number
  maxLength?: number
  pattern?: string
  minProperties?: number
  maxProperties?: number
}

export type OperationProperty = Omit<OperationBaseProperty, 'type'> & {
  type: OperationSchemaType
  minItems?: number
  maxItems?: number
  uniqueItems?: boolean
  items?: OperationBaseProperty
  default?: unknown
}

export type OperationFormValue = string | boolean | null | undefined
export type OperationFormState = Record<string, OperationFormValue>

export function operationPropertyType(property: Pick<OperationProperty, 'type'>): OperationBaseType {
  const type = Array.isArray(property.type) ? property.type.find(value => value !== 'null') : property.type
  return type as OperationBaseType
}

export function isNullableOperationProperty(property: Pick<OperationProperty, 'type'>): boolean {
  return Array.isArray(property.type) && property.type.includes('null')
}

export function initialOperationForm(
  properties: Record<string, unknown>,
): OperationFormState {
  return Object.fromEntries(Object.entries(properties).map(([name, raw]) => {
    const property = raw as OperationProperty
    const type = operationPropertyType(property)
    if (property.default === null && isNullableOperationProperty(property)) return [name, null]
    if (type === 'boolean') {
      return [name, typeof property.default === 'boolean' ? property.default : undefined]
    }
    if (property.default !== undefined) {
      return [name, typeof property.default === 'string' ? property.default : JSON.stringify(property.default)]
    }
    return [name, '']
  }))
}

export function operationParams(
  properties: Record<string, unknown>,
  required: string[],
  state: OperationFormState,
): Record<string, unknown> {
  const params: Record<string, unknown> = {}
  for (const [name, raw] of Object.entries(properties)) {
    const property = raw as OperationProperty
    const type = operationPropertyType(property)
    const value = state[name]
    if (value === null) {
      if (!isNullableOperationProperty(property)) throw new Error(`${name} does not accept null.`)
      params[name] = null
      continue
    }
    if (type === 'boolean') {
      if (typeof value === 'boolean') params[name] = value
      else if (required.includes(name)) throw new Error(`${name} is required.`)
      continue
    }
    if (typeof value !== 'string' || value.trim() === '') {
      if (required.includes(name)) throw new Error(`${name} is required.`)
      continue
    }
    if (type === 'integer' || type === 'number') {
      const parsed = Number(value)
      if (!Number.isFinite(parsed) || (type === 'integer' && !Number.isInteger(parsed))) {
        throw new Error(`${name} must be ${type === 'integer' ? 'an integer' : 'a number'}.`)
      }
      if (property.minimum !== undefined && parsed < property.minimum) throw new Error(`${name} must be at least ${property.minimum}.`)
      if (property.maximum !== undefined && parsed > property.maximum) throw new Error(`${name} must be at most ${property.maximum}.`)
      if (property.enum && !property.enum.includes(parsed)) throw new Error(`${name} is not an allowed value.`)
      params[name] = parsed
      continue
    }
    if (type === 'array') {
      try {
        const trimmed = value.trim()
        const parsed = trimmed.startsWith('[') || trimmed.startsWith('{')
          ? JSON.parse(value)
          : value.split(',').map(item => item.trim()).filter(Boolean)
        if (!Array.isArray(parsed)) throw new Error()
        if (property.minItems !== undefined && parsed.length < property.minItems) throw new Error(`${name} must contain at least ${property.minItems} items.`)
        if (property.maxItems !== undefined && parsed.length > property.maxItems) throw new Error(`${name} must contain at most ${property.maxItems} items.`)
        const items = property.items
        const typed = items?.type ? parsed.map((item, index) => coerceArrayItem(name, index, item, items)) : parsed
        if (property.uniqueItems && new Set(typed.map(item => JSON.stringify(item))).size !== typed.length) throw new Error(`${name} must contain unique items.`)
        params[name] = typed
        continue
      } catch (error) {
        if (error instanceof Error && error.message.startsWith(`${name} `)) throw error
        throw new Error(`${name} must be a JSON array or comma-separated list.`, { cause: error })
      }
    }
    if (type === 'object') {
      try {
        const parsed = JSON.parse(value) as unknown
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error()
        const size = Object.keys(parsed).length
        if (property.minProperties !== undefined && size < property.minProperties) throw new Error(`${name} must contain at least ${property.minProperties} properties.`)
        if (property.maxProperties !== undefined && size > property.maxProperties) throw new Error(`${name} must contain at most ${property.maxProperties} properties.`)
        params[name] = parsed
        continue
      } catch (error) {
        if (error instanceof Error && error.message.startsWith(`${name} `)) throw error
        throw new Error(`${name} must be a JSON object.`, { cause: error })
      }
    }
    if (property.minLength !== undefined && value.length < property.minLength) throw new Error(`${name} must contain at least ${property.minLength} characters.`)
    if (property.maxLength !== undefined && value.length > property.maxLength) throw new Error(`${name} must contain at most ${property.maxLength} characters.`)
    if (property.pattern !== undefined && !new RegExp(property.pattern).test(value)) throw new Error(`${name} has an invalid format.`)
    if (property.enum && !property.enum.includes(value)) throw new Error(`${name} is not an allowed value.`)
    params[name] = value
  }
  return params
}

function coerceArrayItem(name: string, index: number, value: unknown, property: OperationBaseProperty): unknown {
  if (!property.type) return value
  if (value === null) {
    if (Array.isArray(property.type) && property.type.includes('null')) return null
    throw new Error(`${name} item ${index + 1} does not accept null.`)
  }
  const type = operationPropertyType({ type: property.type })
  let parsed = value
  if (type === 'string') {
    if (typeof value !== 'string') throw new Error(`${name} item ${index + 1} must be a string.`)
    if (property.minLength !== undefined && value.length < property.minLength) throw new Error(`${name} item ${index + 1} is too short.`)
    if (property.maxLength !== undefined && value.length > property.maxLength) throw new Error(`${name} item ${index + 1} is too long.`)
    if (property.pattern !== undefined && !new RegExp(property.pattern).test(value)) throw new Error(`${name} item ${index + 1} has an invalid format.`)
  } else if (type === 'number' || type === 'integer') {
    parsed = typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : Number.NaN
    if (!Number.isFinite(parsed) || (type === 'integer' && !Number.isInteger(parsed))) throw new Error(`${name} item ${index + 1} must be ${type === 'integer' ? 'an integer' : 'a number'}.`)
    if (property.minimum !== undefined && (parsed as number) < property.minimum) throw new Error(`${name} item ${index + 1} must be at least ${property.minimum}.`)
    if (property.maximum !== undefined && (parsed as number) > property.maximum) throw new Error(`${name} item ${index + 1} must be at most ${property.maximum}.`)
  } else if (type === 'boolean') {
    if (value === true || value === false) parsed = value
    else if (value === 'true') parsed = true
    else if (value === 'false') parsed = false
    else throw new Error(`${name} item ${index + 1} must be a boolean.`)
  } else if (type === 'object') {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${name} item ${index + 1} must be an object.`)
  } else if (!Array.isArray(value)) throw new Error(`${name} item ${index + 1} must be an array.`)
  if (property.enum && !property.enum.includes(parsed as string | number)) throw new Error(`${name} item ${index + 1} is not an allowed value.`)
  return parsed
}

export function isDestructiveOperation(annotations?: Record<string, unknown>): boolean {
  return annotations?.destructiveHint === true
}
