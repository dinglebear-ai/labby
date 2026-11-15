// The browser receives only the launch URL and an opaque, owner-bound flow ID.
export function providerLogin({request, view, initialFlow, schedule = setTimeout, clear = clearTimeout, now = Date.now}) {
  let controller
  let timer
  let flow = initialFlow
  let recoveries = 0
  let expiry = 0
  let attempts = 0
  let generation = 0
  const stop = () => {
    generation += 1
    clear(timer)
    controller?.abort()
  }
  const fail = message => {
    stop()
    view.error(message || 'Provider sign-in stopped. Start a fresh sign-in to continue.')
  }
  const poll = async expected => {
    if (expected !== generation) return
    if (attempts >= 60 || now() >= expiry) return fail()
    attempts += 1
    try {
      const result = await request('/sign-in/provider/poll', {flow_id: flow}, controller.signal)
      if (expected !== generation) return
      if (result.status === 'complete') {
        stop()
        view.complete()
      } else if (result.status === 'pending') {
        timer = schedule(() => poll(expected), Math.max(5000, Math.min(30000, result.next_poll_ms || 5000)))
      } else fail()
    } catch (_) {
      if (expected !== generation) return
      if (recoveries === 0 && attempts < 60 && now() < expiry) {
        recoveries += 1
        timer = schedule(() => poll(expected), 5000)
      } else fail('Sign-in delivery could not be confirmed. Reset sign-in and sign out before trying again.')
    }
  }
  return {
    async start() {
      stop()
      const expected = generation
      controller = new AbortController()
      attempts = 0
      recoveries = 0
      view.pending()
      try {
        const result = await request('/sign-in/provider/start', {}, controller.signal)
        if (expected !== generation) return
        flow = result.flow_id
        expiry = Math.min(now() + 300000, result.expires_at * 1000)
        if (typeof flow !== 'string' || now() >= expiry) return fail()
        view.showLink(result.authorization_url)
        timer = schedule(() => poll(expected), 5000)
      } catch (error) {
        if (expected === generation) fail(error.message === 'provider_origin_mismatch' ? 'Provider sign-in is misconfigured. Ask your administrator to check the gateway auth origin.' : undefined)
      }
    },
    async cancel() {
      const id = flow
      stop()
      const expected = generation
      controller = new AbortController()
      const cancellation = controller
      const timeout = schedule(() => cancellation.abort(), 6000)
      timer = timeout
      try {
        const result = await request('/sign-in/provider/cancel', id ? {flow_id: id} : {}, cancellation.signal)
        if (expected !== generation) return
        if (result.status !== 'cancelled') throw new Error('reset_unconfirmed')
        flow = undefined
        const message = result.revocation_unconfirmed ? 'Sign-in reset locally. Gateway revocation was not confirmed.' : 'Provider sign-in reset. You can start a fresh sign-in.'
        if (view.reset) view.reset(message)
        else view.error(message)
      } catch (_) {
        if (expected !== generation) return
        view.error('Sign-in reset could not be confirmed. Use Reset sign-in and sign out to try again.')
      } finally { clear(timeout) }
    },
    destroy: stop,
  }
}

export function mountOAuthLogin(doc) {
  const root = doc.querySelector('[data-provider-login]')
  if (!root) return
  const start = root.querySelector('[data-provider-start]')
  const cancel = root.querySelector('[data-provider-cancel]')
  const link = root.querySelector('[data-provider-link]')
  const status = root.querySelector('[data-provider-status]')
  const client = providerLogin({
    initialFlow: root.dataset.flowId || undefined,
    request: async (path, body, signal) => {
      const response = await fetch(path, {
        method: 'POST', credentials: 'same-origin', cache: 'no-store',
        signal: AbortSignal.any([signal, AbortSignal.timeout(12000)]),
        headers: {'content-type': 'application/json', 'x-csrf-token': doc.querySelector('meta[name="csrf-token"]').content},
        body: JSON.stringify(body),
      })
      if (!response.ok) {
        const error = await response.json()
        throw new Error(error.error === 'provider_origin_mismatch' ? 'provider_origin_mismatch' : 'provider_sign_in_failed')
      }
      return response.json()
    },
    view: {
      pending() { start.disabled = true; cancel.hidden = false; cancel.textContent = 'Cancel provider sign-in'; link.hidden = true; link.removeAttribute('href'); status.textContent = 'Preparing provider sign-in…' },
      showLink(url) { link.href = url; link.hidden = false; status.textContent = 'Open the provider sign-in link, then return here.'; link.focus() },
      error(message) { start.disabled = true; cancel.hidden = false; cancel.textContent = 'Reset sign-in and sign out'; link.hidden = true; link.removeAttribute('href'); status.textContent = message },
      reset(message) { start.disabled = false; cancel.hidden = true; link.hidden = true; link.removeAttribute('href'); status.textContent = message },
      complete() { window.location.assign('/gateway') },
    },
  })
  const begin = () => client.start()
  const abandon = () => client.cancel()
  const escape = event => { if (event.key === 'Escape' && !cancel.hidden) abandon() }
  const teardown = () => {
    client.destroy()
    start.removeEventListener('click', begin)
    cancel.removeEventListener('click', abandon)
    root.removeEventListener('keydown', escape)
    window.removeEventListener('pagehide', teardown)
    window.removeEventListener('phx:page-loading-start', teardown)
  }
  start.addEventListener('click', begin)
  cancel.addEventListener('click', abandon)
  root.addEventListener('keydown', escape)
  window.addEventListener('pagehide', teardown)
  window.addEventListener('phx:page-loading-start', teardown)
  return teardown
}
