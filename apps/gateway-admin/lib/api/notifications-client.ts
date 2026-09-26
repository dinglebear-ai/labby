import { z } from 'zod'

const notificationSchema = z.object({
  id: z.string().min(1).max(128),
  createdAtUnixMs: z.number().int().nonnegative(),
  level: z.string().max(32),
  title: z.string().max(256),
  body: z.string().max(8192),
  source: z.string().max(128),
  dedupeKey: z.string().max(2048),
}).strict()

const responseSchema = z.object({
  notifications: z.array(notificationSchema).max(2000),
}).strict()

export type LabbyNotification = z.infer<typeof notificationSchema>

export async function listNotifications(signal?: AbortSignal): Promise<LabbyNotification[]> {
  const response = await fetch('/v1/notifications', {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  })
  const body: unknown = await response.json().catch(() => ({}))
  if (!response.ok) {
    const message = typeof body === 'object' && body !== null && 'message' in body
      ? String((body as { message?: unknown }).message ?? 'notifications unavailable')
      : 'notifications unavailable'
    throw new Error(message)
  }
  return responseSchema.parse(body).notifications
}
