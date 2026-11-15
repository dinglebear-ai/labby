// Only presentation preferences belong in browser storage. The server supplies
// an opaque gateway + verified caller namespace; an absent namespace disables persistence.
const storageKey = namespace => typeof namespace === 'string' && namespace.length > 0 && namespace.length <= 512
  ? `labby:${namespace}:sidebar` : null
export function readCollapsed(storage, namespace) {
  const key = storageKey(namespace)
  if (!key) return false
  try { return storage.getItem(key) === 'true' } catch (_) { return false }
}
export function writeCollapsed(storage, namespace, collapsed) {
  const key = storageKey(namespace)
  if (key) try { storage.setItem(key, String(collapsed)) } catch (_) {}
}
export const WorkspaceChrome = {
  mounted() {
    this.namespace = this.el.dataset.preferenceKey
    // Accessing localStorage itself may throw when storage is denied.
    try { this.storage = window.localStorage } catch (_) { this.storage = null }
    this.collapsed = readCollapsed(this.storage, this.namespace)
    this.syncSidebar = () => {
      const sidebar = this.el.querySelector('.ov-sidebar')
      const toggle = this.el.querySelector('#labby-sidebar-toggle')
      if (!sidebar || !toggle) return
      sidebar.dataset.collapsed = String(this.collapsed)
      toggle.setAttribute('aria-expanded', String(!this.collapsed))
      const label = this.collapsed ? 'Expand navigation' : 'Collapse navigation'
      toggle.setAttribute('aria-label', label)
      toggle.title = label
    }
    this.click = event => {
      if (event.target.closest('#labby-sidebar-toggle')) {
        this.collapsed = !this.collapsed
        writeCollapsed(this.storage, this.namespace, this.collapsed)
        this.syncSidebar()
      }
    }
    this.escape = event => {
      if (event.key !== 'Escape') return
      const more = this.el.querySelector('#labby-mobile-more-menu')
      if (more?.matches(':popover-open')) {
        more.hidePopover()
        this.el.querySelector('#labby-mobile-more')?.focus()
      }
    }
    this.el.addEventListener('click', this.click)
    window.addEventListener('keydown', this.escape)
    this.syncSidebar()
  },
  updated() {
    if (this.namespace !== this.el.dataset.preferenceKey) {
      this.namespace = this.el.dataset.preferenceKey
      this.collapsed = readCollapsed(this.storage, this.namespace)
    }
    this.syncSidebar()
  },
  destroyed() {
    this.el.removeEventListener('click', this.click)
    window.removeEventListener('keydown', this.escape)
  }
}
