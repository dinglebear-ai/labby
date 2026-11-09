const enums = {tab: ['activity', 'logs', 'traces'], window: ['1h', '24h', '7d']}
function storageKey(key) {
  return typeof key === 'string' && /^[a-f0-9]{64}$/.test(key) ? `labby:observe:${key}:v1` : null
}
export function normalizePreferences(value) {
  const result = {}
  if (!value || typeof value !== 'object' || Array.isArray(value)) return result
  for (const [field, allowed] of Object.entries(enums)) {
    if (allowed.includes(value[field])) result[field] = value[field]
  }
  return result
}
export function readPreferences(storage, key) {
  const name = storageKey(key)
  if (!name) return {}
  try {
    const raw = storage.getItem(name)
    return raw && raw.length <= 1024 ? normalizePreferences(JSON.parse(raw)) : {}
  } catch { return {} }
}
export function writePreferences(storage, key, value) {
  const name = storageKey(key)
  if (!name) return false
  try { storage.setItem(name, JSON.stringify(normalizePreferences(value))); return true } catch { return false }
}
export function focusTarget(document, selected, main) {
  return document.getElementById(`observe-inspect-${selected}`) || main
}
export function downloadSnapshot(payload, key, env) {
  if (payload?.key !== key || !storageKey(key) || typeof payload.data !== 'string' || new TextEncoder().encode(payload.data).length > 65536) return false
  const blob = new env.Blob([payload.data], {type: 'application/json;charset=utf-8'})
  const url = env.URL.createObjectURL(blob)
  const anchor = env.document.createElement('a')
  anchor.href = url
  anchor.download = 'labby-observe-loaded-snapshot.json'
  env.document.body.appendChild(anchor)
  try { anchor.click() } finally { anchor.remove(); env.URL.revokeObjectURL(url) }
  return true
}
export default {
  mounted() {
    this.key = this.el.dataset.preferenceKey
    this.selected = null
    this.alive = true
    this.exportRequested = false
    try { this.storage = window.localStorage } catch { this.storage = undefined }
    this.restore()
    this.preferenceEvent = this.handleEvent('observe-preferences-state', payload => {
      if (payload.key === this.key && !this.restoring && !writePreferences(this.storage, this.key, payload.preferences)) this.storageUnavailable()
    })
    this.exportEvent = this.handleEvent('observe-export', payload => {
      if (!this.alive || !this.exportRequested) return
      this.exportRequested = false
      downloadSnapshot(payload, this.key, window)
    })
    this.clickHandler = event => {
      if (event.target.closest('#observe-export')) this.exportRequested = true
    }
    this.keyboard = event => {
      const drawer = this.el.querySelector('#observe-inspector')
      if (!drawer) return
      if (event.key === 'Escape') { event.preventDefault(); this.pushEvent('close_inspector', {}) }
      if (event.key === 'Tab') {
        const items = [...drawer.querySelectorAll('button, a[href], input, select, [tabindex="0"]')]
        const first = items[0], last = items.at(-1)
        if (event.shiftKey && (document.activeElement === first || !drawer.contains(document.activeElement))) { event.preventDefault(); last?.focus() }
        else if (!event.shiftKey && (document.activeElement === last || !drawer.contains(document.activeElement))) { event.preventDefault(); first?.focus() }
      }
    }
    this.el.addEventListener('click', this.clickHandler)
    document.addEventListener('keydown', this.keyboard)
    this.updated()
  },
  restore() {
    this.restoring = true
    this.pushEvent('observe_preferences', {preferences: readPreferences(this.storage, this.key)}, () => { this.restoring = false })
    if (!this.storage) this.storageUnavailable()
  },
  storageUnavailable() {
    if (this.warned) return
    this.warned = true
    this.pushEvent('observe_storage_unavailable', {})
  },
  updated() {
    const key = this.el.dataset.preferenceKey
    if (key !== this.key) {
      this.key = key
      this.selected = null
      this.exportRequested = false
      this.warned = false
      this.restore()
    }
    const drawer = this.el.querySelector('#observe-inspector')
    if (drawer && drawer.dataset.selected !== this.selected) {
      this.selected = drawer.dataset.selected
      drawer.querySelector('#observe-close')?.focus()
    } else if (!drawer && this.selected) {
      focusTarget(document, this.selected, this.el).focus()
      this.selected = null
    }
  },
  disconnected() { this.exportRequested = false },
  destroyed() {
    this.alive = false
    document.removeEventListener('keydown', this.keyboard)
    this.el.removeEventListener('click', this.clickHandler)
    this.removeHandleEvent(this.preferenceEvent)
    this.removeHandleEvent(this.exportEvent)
    this.storage = undefined
    this.key = null
    this.selected = null
    this.exportRequested = false
  }
}
