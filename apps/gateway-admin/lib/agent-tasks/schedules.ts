import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { performServiceAction, type ServiceActionError } from '@/lib/api/service-action-client'
import type { AgentView, TaskResult, TaskView } from './client'

export type ScheduleSpec =
  | { kind: 'once'; at: number }
  | { kind: 'interval'; every_ms: number }
  | { kind: 'daily'; hour: number; minute: number; timezone: string }
  | { kind: 'weekly'; weekdays: number[]; hour: number; minute: number; timezone: string }
  | { kind: 'cron'; expression: string; timezone: string }
export type RetryPolicy = { max_retries: number; backoff_ms: number }
export type TaskTemplate = { owner_kind: string; owner_id: string; agent_id: string; input: string; project_id?: string }
export type TaskSchedule = { schedule_id: string; name: string; owner_kind: string; owner_id: string; agent_id: string; schedule: ScheduleSpec; armed: boolean; next_run_at: number | null; revision: number; last_task_id: string | null; last_error_kind: string | null; missed_run_policy: string; dst_policy: string; retry_policy?: RetryPolicy; next_retry_at?: number | null }
export type TaskScheduleDetail = TaskSchedule & { task_template: TaskTemplate }
export type CreateTaskSchedule = TaskTemplate & { schedule_id: string; name: string; schedule: ScheduleSpec; armed: boolean; retry_policy?: RetryPolicy }
export type EditTaskSchedule = { schedule_id: string; name: string; agent_id: string; input: string; schedule: ScheduleSpec; retry_policy?: RetryPolicy }

export function taskAction<T>(name: string, params: object = {}, signal?: AbortSignal): Promise<T> {
  return performServiceAction<T, ServiceActionError>({ serviceLabel: 'Tasks', url: `${normalizeGatewayApiBase()}/tasks`, action: name, params, signal,
    createError: (message, status, code, param) => Object.assign(new Error(message), { name: 'TaskActionError', status, code, param }) })
}
async function allPages<T>(service: 'tasks' | 'agents', action: string, key: 'schedules' | 'agents', signal?: AbortSignal): Promise<T[]> {
  const rows: T[] = []
  let cursor: string | undefined
  const visited = new Set<string>()
  for (let page = 0; page < 100; page++) {
    const params = service === 'agents'
      ? cursor ? { cursor, limit: '100' } : { limit: '100' }
      : cursor ? { cursor } : {}
    const result: { schedules?: T[]; agents?: T[]; next_cursor?: string | null } = service === 'tasks'
      ? await taskAction<{ schedules?: T[]; agents?: T[]; next_cursor?: string | null }>(action, params, signal)
      : await performServiceAction<{ agents: T[]; next_cursor?: string | null }, ServiceActionError>({ serviceLabel: 'Agents', url: `${normalizeGatewayApiBase()}/agents`, action, params, signal, createError: (message, status, code, param) => Object.assign(new Error(message), { status, code, param }) })
    rows.push(...(result[key] ?? []))
    if (!result.next_cursor) return rows
    if (visited.has(result.next_cursor)) throw new Error('The server repeated a pagination cursor. Refresh to retry.')
    cursor = result.next_cursor
    visited.add(cursor)
  }
  throw new Error('The task inventory exceeds the bounded listing limit.')
}
export const listTaskSchedules = (signal?: AbortSignal) => allPages<TaskSchedule>('tasks', 'tasks.schedule_list', 'schedules', signal)
export const listScheduleAgents = (signal?: AbortSignal) => allPages<AgentView>('agents', 'agents.list', 'agents', signal)
export const getTaskSchedule = (id: string, signal?: AbortSignal) => taskAction<TaskScheduleDetail>('tasks.schedule_get', { schedule_id: id }, signal)
export const createTaskSchedule = (value: CreateTaskSchedule, signal?: AbortSignal) => taskAction<TaskSchedule>('tasks.schedule_create', value, signal)
export const editTaskSchedule = (value: EditTaskSchedule, signal?: AbortSignal) => taskAction<TaskSchedule>('tasks.schedule_edit', value, signal)
export const armTaskSchedule = (id: string, signal?: AbortSignal) => taskAction<TaskSchedule>('tasks.schedule_arm', { schedule_id: id }, signal)
export const pauseTaskSchedule = (id: string, signal?: AbortSignal) => taskAction<TaskSchedule>('tasks.schedule_pause', { schedule_id: id }, signal)
export const deleteTaskSchedule = (id: string, signal?: AbortSignal) => taskAction<{ deleted: boolean }>('tasks.schedule_delete', { schedule_id: id }, signal)
export const runTaskSchedule = (id: string, idempotencyKey: string, signal?: AbortSignal) => taskAction<{ state: string }>('tasks.schedule_run_now', { schedule_id: id, idempotency_key: idempotencyKey }, signal)
export const getScheduledTask = (id: string, signal?: AbortSignal) => taskAction<TaskView>('tasks.get', { task_id: id }, signal)
export const getScheduledTaskResult = (id: string, signal?: AbortSignal) => taskAction<TaskResult>('tasks.result', { task_id: id }, signal)

const weekdays = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
export function scheduleLabel(spec: ScheduleSpec): string {
  if (spec.kind === 'once') return `Once · ${new Date(spec.at).toLocaleString()}`
  if (spec.kind === 'interval') return `Every ${spec.every_ms / 60_000} min`
  if (spec.kind === 'cron') return spec.expression
  const time = `${String(spec.hour).padStart(2, '0')}:${String(spec.minute).padStart(2, '0')}`
  return `${spec.kind === 'daily' ? 'Daily' : spec.weekdays.map(day => weekdays[day]).join(', ')} · ${time}`
}
export function scheduleTimezone(spec: ScheduleSpec) { return 'timezone' in spec ? spec.timezone : spec.kind === 'once' ? 'Browser local time' : 'Elapsed time' }
