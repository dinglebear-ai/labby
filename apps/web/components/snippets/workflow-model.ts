import { ownPath, safeSegment, walkMapping } from './workflow-selectors'
import type { ParameterSchema } from './tool-parameter-model'
import { redactWorkflowValue, sensitiveWorkflowName, WORKFLOW_REDACTED } from './workflow-redaction'
export { suggestWorkflowMapping } from './workflow-suggestions'
export interface WorkflowStep { id: string; tool: string; mapping: Record<string, unknown>; dependsOn: string[] }
export interface WorkflowPlan { version: 1; steps: WorkflowStep[] }
export interface WorkflowValidation { valid: boolean; errors: string[]; waves: string[][]; dependencies: Record<string, string[]> }
export interface WorkflowPreviewStep { id: string; tool: string; params: Record<string, unknown>; unresolved: string[]; dependsOn: string[]; wave: number }
export interface WorkflowPreview { valid: boolean; errors: string[]; waves: string[][]; steps: WorkflowPreviewStep[]; note: string }

export function validateWorkflow(plan: WorkflowPlan): WorkflowValidation {
  const errors: string[] = []
  const dependencies: Record<string, string[]> = Object.create(null)
  if (plan?.version !== 1 || !Array.isArray(plan.steps) || !plan.steps.length || plan.steps.length > 128) return { valid: false, errors: ['Workflow version 1 requires 1–128 steps.'], waves: [], dependencies }
  const ids = new Set<string>()
  for (const step of plan.steps) {
    if (!step || typeof step.id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(step.id) || !safeSegment(step.id)) { errors.push('Each step requires a safe id of 1–64 letters, numbers, underscores or hyphens.'); continue }
    if (ids.has(step.id)) errors.push(`${step.id}: duplicate step id.`)
    ids.add(step.id)
  }
  for (const step of plan.steps) {
    if (!step || typeof step.id !== 'string') continue
    if (typeof step.tool !== 'string' || !/^[^\s:]+::[^\s:]+$/.test(step.tool) || step.tool.length > 1024) errors.push(`${step.id}: tool must be an exact upstream::tool identifier.`)
    const deps = new Set<string>()
    if (!Array.isArray(step.dependsOn) || !step.dependsOn.every((id) => typeof id === 'string')) errors.push(`${step.id}: dependsOn must be an array of step ids.`)
    else for (const id of step.dependsOn) deps.add(id)
    try {
      if (!step.mapping || Array.isArray(step.mapping) || typeof step.mapping !== 'object') throw new Error('Mapping must be a JSON object.')
      walkMapping(step.mapping, (selector, expression) => { if (selector.stepId) deps.add(selector.stepId); return expression })
    } catch (error) { errors.push(`${step.id}: ${error instanceof Error ? error.message : 'Invalid mapping.'}`) }
    for (const id of deps) {
      if (!ids.has(id)) errors.push(`${step.id}: unknown dependency "${id}".`)
      if (id === step.id) errors.push(`${step.id}: a step cannot depend on itself.`)
    }
    dependencies[step.id] = [...deps].sort()
  }
  const waves: string[][] = []
  const completed = new Set<string>()
  while (completed.size < ids.size) {
    const wave = plan.steps.filter((step) => step && ids.has(step.id) && !completed.has(step.id) && dependencies[step.id]?.every((id) => completed.has(id))).map((step) => step.id)
    if (!wave.length) { if (!errors.length) errors.push('Workflow contains a dependency cycle.'); break }
    waves.push(wave)
    for (const id of wave) completed.add(id)
  }
  return { valid: errors.length === 0, errors, waves, dependencies }
}

/** Resolves input selectors only; output selectors stay visible until real execution. */
export function previewWorkflow(plan: WorkflowPlan, input: Record<string, unknown>, schemas: Record<string, ParameterSchema> = {}): WorkflowPreview {
  const validation = validateWorkflow(plan)
  const steps = validation.valid ? plan.steps.map((step) => {
    const unresolved: string[] = []
    const params = walkMapping(step.mapping, (selector, expression) => {
      if (selector.source === 'steps') { unresolved.push(expression); return expression }
      try { return selector.path.some(sensitiveWorkflowName) ? WORKFLOW_REDACTED : redactWorkflowValue(ownPath(input, selector.path)) } catch { unresolved.push(expression); return expression }
    }) as Record<string, unknown>
    return { id: step.id, tool: step.tool, params: redactWorkflowValue(params, schemas[step.id]) as Record<string, unknown>, unresolved: [...new Set(unresolved)], dependsOn: validation.dependencies[step.id], wave: validation.waves.findIndex((wave) => wave.includes(step.id)) }
  }) : []
  return { ...validation, steps, note: 'Static mapping preview only. Output references resolve during execution; failed prerequisites skip dependent steps.' }
}

/** Generates a saved snippet arrow using deferred, failure-isolated wave batches. */
export function generateWorkflowCode(plan: WorkflowPlan): string {
  const validation = validateWorkflow(plan)
  if (!validation.valid) throw new Error(validation.errors.join('\n'))
  const execution = { steps: plan.steps, waves: validation.waves, dependencies: validation.dependencies }
  return `async () => {
  const plan = ${JSON.stringify(execution)};
  const results = Object.create(null);
  const unsafe = new Set(['__proto__', 'prototype', 'constructor']);
  const ownPath = (value, path) => {
    for (const key of path) {
      if (!key || unsafe.has(key) || typeof value !== 'object' || value === null || !Object.prototype.hasOwnProperty.call(value, key)) throw new Error('Missing or unsafe selector property: ' + key);
      value = value[key];
    }
    return value;
  };
  const resolve = (value) => {
    if (typeof value === 'string' && (value.startsWith('$input.') || value.startsWith('$steps.'))) {
      const parts = value.split('.');
      const source = parts.shift();
      if (source === '$input') return ownPath(input, parts);
      const id = parts.shift();
      return ownPath(results[id].value, parts);
    }
    if (Array.isArray(value)) return value.map(resolve);
    if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, resolve(entry)]));
    return value;
  };
  for (const wave of plan.waves) {
    const runnable = [];
    for (const id of wave) {
      const blocked = plan.dependencies[id].filter((dependency) => results[dependency].status !== 'succeeded');
      if (blocked.length) results[id] = { id, status: 'skipped', dependencies: blocked, reason: 'Prerequisite did not succeed' };
      else runnable.push(plan.steps.find((step) => step.id === id));
    }
    if (!runnable.length) continue;
    const batch = await codemode.batch(runnable.map((step) => () => callTool(step.tool, resolve(step.mapping))));
    for (const entry of batch.ok) { const id = runnable[entry.i].id; results[id] = { id, status: 'succeeded', value: entry.value }; }
    for (const entry of batch.failed) { const id = runnable[entry.i].id; results[id] = { id, status: 'failed', error: entry.error }; }
  }
  const all_ok = plan.steps.every((step) => results[step.id].status === 'succeeded');
  return { steps: plan.steps.map((step) => ({ tool: step.tool, ...results[step.id] })), all_ok, ok: all_ok };
}`
}

/** Recognizes builder output without executing source or trusting serialized scheduler data. */
export function readWorkflowPlan(bodyOrCode: string): WorkflowPlan | undefined {
  if (typeof bodyOrCode !== 'string') return undefined
  const trimmed = bodyOrCode.trim()
  const fenceLines = [...trimmed.matchAll(/^```[^\n]*$/gm)]
  const fenced = trimmed.match(/^```(?:js|javascript)\s*\r?\n([\s\S]*?)\r?\n```\s*$/m)
  if (fenceLines.length && (fenceLines.length !== 2 || !fenced)) return undefined
  const source = (fenced ? fenced[1] : trimmed).trim()
  const literal = source.match(/^ {2}const plan = (\{[^\n]*\});$/m)?.[1]
  if (!literal) return undefined
  try {
    const serialized: unknown = JSON.parse(literal)
    if (typeof serialized !== 'object' || serialized === null || Array.isArray(serialized) || !Object.prototype.hasOwnProperty.call(serialized, 'steps')) return undefined
    const plan: WorkflowPlan = { version: 1, steps: (serialized as { steps: WorkflowStep[] }).steps }
    if (!validateWorkflow(plan).valid || generateWorkflowCode(plan).trim() !== source) return undefined
    return plan
  } catch { return undefined }
}

/** Rename an identity and its edges without changing literal strings or the source plan. */
export function renameWorkflowStep(plan: WorkflowPlan, oldId: string, newId: string): WorkflowPlan {
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(newId) || !safeSegment(newId)) throw new Error('New step id must contain 1–64 safe letters, numbers, underscores or hyphens.')
  if (!plan.steps.some((step) => step.id === oldId)) throw new Error(`Unknown step "${oldId}".`)
  if (oldId !== newId && plan.steps.some((step) => step.id === newId)) throw new Error(`Duplicate step id "${newId}".`)
  return {
    ...plan,
    steps: plan.steps.map((step) => ({
      ...step,
      id: step.id === oldId ? newId : step.id,
      dependsOn: step.dependsOn.map((id) => id === oldId ? newId : id),
      mapping: walkMapping(step.mapping, (selector, expression) => selector.source === 'steps' && selector.stepId === oldId
        ? `$steps.${newId}${selector.path.length ? `.${selector.path.join('.')}` : ''}` : expression) as Record<string, unknown>,
    })),
  }
}
