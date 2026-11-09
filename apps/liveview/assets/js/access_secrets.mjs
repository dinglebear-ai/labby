export function newInvitationToken(crypto) {
  const bytes = new Uint8Array(32)
  crypto.getRandomValues(bytes)
  if (!bytes.some(byte => byte !== 0)) throw new Error('Token generation failed')
  return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')
}

export default {
  mounted() {
    this.ready = false
    this.secret = null
    this.nonce = null
    this.submitting = false
    this.revealed = false
    this.clear = () => {
      this.secret = null
      this.nonce = null
      this.submitting = false
      this.revealed = false
      for (const input of this.el.querySelectorAll('#access-field-token, #access-created-secret')) {
        input.value = ''
        input.type = 'password'
      }
      const output = this.el.querySelector('[data-access-secret-output]')
      if (output) output.hidden = true
    }
    this.status = message => {
      const target = this.el.querySelector('[data-access-secret-status]')
      if (target) target.textContent = message
    }
    this.click = async event => {
      if (event.target.closest('button[phx-value-action]') && !this.ready) {
        event.preventDefault()
        event.stopImmediatePropagation()
        return
      }
      if (event.target.closest('[data-access-generate]')) {
        try {
          this.el.querySelector('#access-field-token').value = newInvitationToken(window.crypto)
          this.status('Token generated. It is sent only when you confirm creation.')
        } catch { this.status('Token generation failed. Supply a private 64-character hexadecimal token.') }
      }
      if (event.target.closest('[data-access-forget]')) this.clear()
      if (event.target.closest('[data-access-reveal]') && this.secret && !this.revealed) {
        this.el.querySelector('#access-created-secret').type = 'text'
        this.revealed = true
        event.target.disabled = true
      }
      if (event.target.closest('[data-access-copy]') && this.secret) {
        const status = this.el.querySelector('[data-access-copy-status]')
        try {
          await navigator.clipboard.writeText(this.secret)
          this.clear()
          status.textContent = 'Token copied. This browser has forgotten it.'
          status.parentElement.hidden = false
        } catch { status.textContent = 'Invitation created. Clipboard access failed. Copy the revealed token privately or close to forget it.' }
      }
    }
    this.submit = event => {
      const form = event.target.closest('#access-action-form')
      if (!form) return
      if (!this.ready) {
        event.preventDefault()
        event.stopImmediatePropagation()
        return
      }
      const token = form.querySelector('#access-field-token')
      if (!token) return
      event.preventDefault()
      event.stopImmediatePropagation()
      if (this.submitting || !form.reportValidity()) return
      const params = {}
      for (const [key, value] of new FormData(form)) {
        const field = /^access_request\[([a-z_]+)\]$/.exec(key)
        if (field) params[field[1]] = value
      }
      params.token = token.value
      try {
        this.nonce = newInvitationToken(window.crypto).slice(0, 32)
      } catch { this.status('Secure submission is unavailable. Try again when browser cryptography is available.'); return }
      this.secret = form.dataset.action === 'access.team_invitation.create' ? params.token : null
      this.submitting = true
      this.pushEvent('access_submit', {access_request: params, confirmation: 'yes', nonce: this.nonce})
      if (token) token.value = ''
    }
    this.keydown = event => {
      const menu = this.el.querySelector('#access-actions')
      if (event.key === 'Escape' && menu?.open && menu.contains(event.target)) {
        menu.open = false
        menu.querySelector('summary').focus()
      }
    }
    this.el.addEventListener('click', this.click)
    this.el.addEventListener('submit', this.submit, true)
    this.el.addEventListener('keydown', this.keydown)
    this.handleEvent('access-secret-clear', this.clear)
    this.handleEvent('access-ready', () => {
      this.ready = true
      const status = this.el.querySelector('#access-connection-status')
      if (status) status.hidden = true
    })
    this.handleEvent('access-secret-result', ({nonce, accepted}) => {
      if (!nonce || nonce !== this.nonce) return
      this.submitting = false
      if (accepted && this.secret) {
        const output = this.el.querySelector('[data-access-secret-output]')
        output.hidden = false
        this.el.querySelector('#access-created-secret').value = this.secret
        this.el.querySelector('[data-access-reveal]').disabled = false
        this.el.querySelector('[data-access-copy-status]').textContent = ''
      } else this.clear()
    })
  },
  disconnected() {
    this.ready = false
    this.clear()
    for (const button of this.el.querySelectorAll('button[phx-value-action], #access-submit')) button.disabled = true
    const status = this.el.querySelector('#access-connection-status')
    if (status) status.hidden = false
  },
  reconnected() { this.pushEvent('access_refresh', {card: 'help'}) },
  destroyed() {
    this.clear()
    this.el.removeEventListener('click', this.click)
    this.el.removeEventListener('submit', this.submit, true)
    this.el.removeEventListener('keydown', this.keydown)
  }
}
