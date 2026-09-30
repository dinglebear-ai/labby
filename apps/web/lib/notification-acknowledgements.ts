'use client'

import { useEffect, useSyncExternalStore } from 'react'
import { useBrowserSession } from '@/lib/auth/session'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'

export type GatewayNotification = { key: string; fingerprint: string; gatewayName: string; message: string }
type RecordState = GatewayNotification & { source: string; active: boolean; occurrence: number; acknowledged?: number }
export type NotificationLedger = Record<string, RecordState>
const EMPTY: NotificationLedger = {}
const stores = new Map<string, { records: NotificationLedger; listeners: Set<() => void> }>()
const storageKey = (scope: string) => `labby-notification-acks-v1:${encodeURIComponent(scope)}`

/** Backend fields define identity; polling time never creates a new occurrence. */
export function reconcileNotifications(previous: NotificationLedger, source: string, observations: readonly GatewayNotification[]): NotificationLedger {
  const next = { ...previous }
  const keys = new Set(observations.map(item => item.key))
  for (const [key, record] of Object.entries(previous)) {
    if (record.source === source && record.active && !keys.has(key)) next[key] = { ...record, active: false }
  }
  for (const item of observations) {
    const old = previous[item.key]
    const recurring = !old?.active || old.fingerprint !== item.fingerprint
    next[item.key] = { ...old, ...item, source, active: true, occurrence: recurring ? (old?.occurrence ?? 0) + 1 : old.occurrence }
  }
  const bounded = boundInactiveNotifications(next)
  return JSON.stringify(bounded) === JSON.stringify(previous) ? previous : bounded
}

export function boundInactiveNotifications(records: NotificationLedger): NotificationLedger {
  const entries = Object.entries(records)
  const inactive = entries.filter(([, record]) => !record.active)
  if (inactive.length <= 256) return records
  const retained = new Set(inactive.slice(-256).map(([key]) => key))
  return Object.fromEntries(entries.filter(([key, record]) => record.active || retained.has(key)))
}

export function notificationScope(apiBase: string, principal: string, organization: string): string {
  return JSON.stringify([apiBase, principal, organization])
}

export function acknowledgeNotifications(records: NotificationLedger): NotificationLedger {
  return Object.fromEntries(Object.entries(records).map(([key, record]) => [key, record.active ? { ...record, acknowledged: record.occurrence } : record]))
}
export function unreadNotifications(records: NotificationLedger): GatewayNotification[] {
  return Object.values(records).filter(record => record.active && record.acknowledged !== record.occurrence)
}

function readLedger(raw: string | null): NotificationLedger {
  try {
    const saved: unknown = JSON.parse(raw ?? '{}')
    if (!saved || typeof saved !== 'object' || Array.isArray(saved)) return {}
    return Object.fromEntries(Object.entries(saved).filter(([key, value]) => value && typeof value === 'object' && value.key === key && typeof value.source === 'string' && typeof value.fingerprint === 'string' && typeof value.gatewayName === 'string' && typeof value.message === 'string' && typeof value.active === 'boolean' && Number.isSafeInteger(value.occurrence) && value.occurrence > 0 && (value.acknowledged === undefined || Number.isSafeInteger(value.acknowledged))).slice(-1024))
  } catch { return {} }
}
function getStore(scope: string) {
  let store = stores.get(scope)
  if (!store) {
    let records: NotificationLedger = {}
    if (typeof window !== 'undefined') {
      try { records = readLedger(window.localStorage.getItem(storageKey(scope))) } catch { /* Storage may be unavailable. */ }
    }
    store = { records, listeners: new Set() }
    stores.set(scope, store)
  }
  return store
}
function update(scope: string, records: NotificationLedger) {
  const store = getStore(scope)
  if (store.records === records) return
  store.records = records
  try { window.localStorage.setItem(storageKey(scope), JSON.stringify(records)) } catch { /* Session dismissal still works without storage. */ }
  store.listeners.forEach(listener => listener())
}

/** Each producer owns a stable source; consumers share the authority's entire ledger. */
export function useGatewayNotifications(source?: string, observations?: readonly GatewayNotification[]) {
  const session = useBrowserSession()
  const scope = session.status === 'authenticated' ? notificationScope(normalizeGatewayApiBase(), session.user.sub, session.authority?.organizationId ?? '') : session.status
  const store = getStore(scope)
  const records = useSyncExternalStore(listener => { store.listeners.add(listener); return () => { store.listeners.delete(listener) } }, () => store.records, () => EMPTY)
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== storageKey(scope)) return
      store.records = readLedger(event.newValue)
      store.listeners.forEach(listener => listener())
    }
    window.addEventListener('storage', onStorage)
    return () => window.removeEventListener('storage', onStorage)
  }, [scope, store])
  const serialized = observations === undefined ? undefined : JSON.stringify(observations)
  useEffect(() => {
    if (source && serialized !== undefined) update(scope, reconcileNotifications(getStore(scope).records, source, JSON.parse(serialized)))
  }, [scope, source, serialized])
  return {
    notifications: session.status === 'authenticated' && session.isAdmin ? unreadNotifications(records) : [],
    isDismissed: (key: string) => { const record = records[key]; return Boolean(record?.active && record.acknowledged === record.occurrence) },
    clearAll: () => update(scope, acknowledgeNotifications(getStore(scope).records)),
  }
}
