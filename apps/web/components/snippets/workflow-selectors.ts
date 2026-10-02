export interface WorkflowSelector { source: 'input' | 'steps'; stepId?: string; path: string[] }
const forbidden = new Set(['__proto__', 'prototype', 'constructor'])
export function safeSegment(segment: string): boolean {
  return segment.length > 0 && !forbidden.has(segment)
}
export function parseWorkflowSelector(value: string): WorkflowSelector | undefined {
  if (!value.startsWith('$input.') && !value.startsWith('$steps.')) return undefined
  const parts = value.split('.')
  const source = parts.shift() === '$input' ? 'input' : 'steps'
  const stepId = source === 'steps' ? parts.shift() : undefined
  if ((source === 'steps' && (!stepId || !safeSegment(stepId))) || (source === 'input' && !parts.length) || !parts.every(safeSegment)) {
    throw new Error(`Invalid or unsafe selector "${value}".`)
  }
  return { source, stepId, path: parts }
}
export function ownPath(value: unknown, path: string[]): unknown {
  let current = value
  for (const segment of path) {
    if (!safeSegment(segment) || typeof current !== 'object' || current === null || !Object.prototype.hasOwnProperty.call(current, segment)) {
      throw new Error(`Missing or unsafe own property "${segment}".`)
    }
    current = (current as Record<string, unknown>)[segment]
  }
  return current
}
/** JSON mappings only. Bounds prevent pathological editor inputs from blocking the UI. */
export function walkMapping(value: unknown, visit: (selector: WorkflowSelector, expression: string) => unknown, depth = 0, seen = new Set<object>()): unknown {
  if (depth > 32) throw new Error('Mapping nesting exceeds 32 levels.')
  if (typeof value === 'string') {
    const selector = parseWorkflowSelector(value)
    return selector ? visit(selector, value) : value
  }
  if (value === null || typeof value === 'boolean' || (typeof value === 'number' && Number.isFinite(value))) return value
  if (typeof value !== 'object') throw new Error('Mappings must contain JSON values.')
  if (seen.has(value)) throw new Error('Mappings cannot contain circular values.')
  seen.add(value)
  try {
    if (Array.isArray(value)) return value.map((item) => walkMapping(item, visit, depth + 1, seen))
    if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) throw new Error('Mappings must contain plain JSON objects.')
    return Object.fromEntries(Object.entries(value).map(([key, entry]) => {
      if (!safeSegment(key)) throw new Error(`Unsafe mapping key "${key}".`)
      return [key, walkMapping(entry, visit, depth + 1, seen)]
    }))
  } finally { seen.delete(value) }
}
