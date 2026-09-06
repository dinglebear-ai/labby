export type OperationProperty = {
  type?: string
  description?: string
  enum?: unknown[]
  minimum?: number
  maximum?: number
  items?: { type?: string; enum?: unknown[] }
  default?: unknown
}

export type OperationFormValue = string | boolean | undefined
export type OperationFormState = Record<string, OperationFormValue>

export function initialOperationForm(
  properties: Record<string, unknown>,
): OperationFormState {
  return Object.fromEntries(Object.entries(properties).map(([name, raw]) => {
    const property = raw as OperationProperty
    if (property.type === 'boolean') {
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
    const value = state[name]
    if (property.type === 'boolean') {
      if (typeof value === 'boolean') params[name] = value
      else if (required.includes(name)) throw new Error(`${name} is required.`)
      continue
    }
    if (typeof value !== 'string' || value.trim() === '') {
      if (required.includes(name)) throw new Error(`${name} is required.`)
      continue
    }
    if (property.type === 'integer' || property.type === 'number') {
      const parsed = Number(value)
      if (!Number.isFinite(parsed) || (property.type === 'integer' && !Number.isInteger(parsed))) {
        throw new Error(`${name} must be ${property.type === 'integer' ? 'an integer' : 'a number'}.`)
      }
      if (property.minimum !== undefined && parsed < property.minimum) throw new Error(`${name} must be at least ${property.minimum}.`)
      if (property.maximum !== undefined && parsed > property.maximum) throw new Error(`${name} must be at most ${property.maximum}.`)
      params[name] = parsed
      continue
    }
    if (property.type === 'array') {
      try {
        const trimmed = value.trim()
        const parsed = trimmed.startsWith('[') || trimmed.startsWith('{')
          ? JSON.parse(value)
          : value.split(',').map(item => item.trim()).filter(Boolean)
        if (!Array.isArray(parsed)) throw new Error()
        params[name] = parsed
        continue
      } catch { throw new Error(`${name} must be a JSON array or comma-separated list.`) }
    }
    if (property.type === 'object') {
      try {
        const parsed = JSON.parse(value) as unknown
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error()
        params[name] = parsed
        continue
      } catch { throw new Error(`${name} must be a JSON object.`) }
    }
    params[name] = value
  }
  return params
}

export function isDestructiveOperation(annotations?: Record<string, unknown>): boolean {
  return annotations?.destructiveHint === true
}
