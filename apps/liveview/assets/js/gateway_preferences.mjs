const fields = {
  layout: ['table', 'cards', 'list'],
  lens: ['all', 'enabled', 'disabled', 'connected', 'disconnected', 'attention'],
  source: ['all', 'in_process', 'custom_gateway', 'unknown'],
  sort: ['name', 'source', 'enabled', 'connected', 'tools', 'resources', 'prompts', 'skills'],
  direction: ['ascending', 'descending']
}
const columns = ['source', 'status', 'tools', 'resources', 'prompts', 'skills']
function storageKey(key) {
  return typeof key === 'string' && /^[a-f0-9]{64}$/.test(key) ? `labby:gateway:${key}:v1` : null
}
export function normalizePreferences(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
  const result = {}
  for (const [field, allowed] of Object.entries(fields)) {
    if (allowed.includes(value[field])) result[field] = value[field]
  }
  if (typeof value.query === 'string' && value.query.length <= 256) result.query = value.query
  if (Array.isArray(value.columns) && value.columns.length <= 12) {
    result.columns = [...new Set(value.columns.filter(column => columns.includes(column)))]
  }
  return result
}
export function readPreferences(storage, key) {
  const name = storageKey(key)
  if (!name) return {}
  try {
    const raw = storage.getItem(name)
    if (!raw || raw.length > 4096) return {}
    return normalizePreferences(JSON.parse(raw))
  } catch {
    return {}
  }
}
export function writePreferences(storage, key, value) {
  const name = storageKey(key)
  if (!name) return false
  try {
    storage.setItem(name, JSON.stringify(normalizePreferences(value)))
    return true
  } catch {
    return false
  }
}
export default {
  mounted() {
    this.key = this.el.dataset.preferenceKey
    try { this.storage = window.localStorage } catch { this.storage = undefined }
    this.restoring = true
    this.stateEvent = this.handleEvent('gateway-preferences-state', payload => {
      if (payload.key !== this.key || this.restoring) return
      if (!writePreferences(this.storage, this.key, payload.preferences)) this.storageUnavailable()
    })
    this.pushEvent('gateway_preferences', {preferences: readPreferences(this.storage, this.key)}, () => {
      this.restoring = false
    })
    this.closeMenus = event => {
      const menus = this.el.querySelectorAll('details[open]')
      for (const menu of menus) {
        if (event.type === 'keydown' && event.key === 'Escape') {
          menu.open = false
          menu.querySelector('summary')?.focus()
        } else if (event.type === 'pointerdown' && !menu.contains(event.target)) {
          menu.open = false
        }
      }
    }
    this.focusMenu = event => {
      if (event.target.matches('details') && event.target.open) {
        for (const menu of this.el.querySelectorAll('details[open]')) {
          if (menu !== event.target) menu.open = false
        }
      }
    }
    document.addEventListener('pointerdown', this.closeMenus)
    document.addEventListener('keydown', this.closeMenus)
    this.el.addEventListener('toggle', this.focusMenu, true)
    if (!this.storage) this.storageUnavailable()
  },
  storageUnavailable() {
    if (this.warned) return
    this.warned = true
    this.pushEvent('gateway_storage_unavailable', {})
  },
  updated() {
    const key = this.el.dataset.preferenceKey
    if (key === this.key) return
    this.key = key
    this.warned = false
    this.restoring = true
    this.pushEvent('gateway_preferences', {preferences: readPreferences(this.storage, key)}, () => {
      this.restoring = false
    })
  },
  destroyed() {
    document.removeEventListener('pointerdown', this.closeMenus)
    document.removeEventListener('keydown', this.closeMenus)
    this.el.removeEventListener('toggle', this.focusMenu, true)
    this.removeHandleEvent(this.stateEvent)
    this.storage = undefined
    this.key = null
  }
}
