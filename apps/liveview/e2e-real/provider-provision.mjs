import {request as httpRequest} from 'node:http'
import {request as httpsRequest} from 'node:https'
import {createHash, randomBytes} from 'node:crypto'

export function requireWrongIssuerRejection(response) {
  if (response?.status !== 401) {
    throw Object.assign(new Error('PROVIDER_WRONG_ISSUER_EXPECTED_401'), {
      requestMetadata: response?.metadata,
    })
  }
}

// Direct public HTTP contracts only. Cookies remain in this server-side helper.
export async function provisionProviderPrincipals({issuer, gateway = 'http://127.0.0.1:4329', onReceipt = () => {}}) {
  let requests = 0
  const sessions = []
  let provisioningFailure

  async function call(url, {method = 'GET', body, cookie, csrf, origin} = {}) {
    if (++requests > 60) throw new Error('PROVIDER_PROVISION_REQUEST_BUDGET')
    const target = new URL(url)
    if (![gateway, issuer.origin].includes(target.origin)) {
      throw new Error('PROVIDER_PROVISION_ORIGIN_DENIED')
    }
    return new Promise((resolve, reject) => {
      const headers = {}
      if (body !== undefined) {
        headers['content-type'] = typeof body === 'string'
          ? 'application/x-www-form-urlencoded'
          : 'application/json'
      }
      if (cookie) headers.cookie = cookie
      if (csrf) headers['x-csrf-token'] = csrf
      if (origin) headers.origin = origin
      const request = target.protocol === 'https:' ? httpsRequest : httpRequest
      const metadata = {method, path: target.pathname, stage: 'request', status: null}
      const req = request(target, {method, headers, ca: issuer.ca, timeout: 5000}, res => {
        metadata.status = res.statusCode
        const callbackHtml = target.origin === gateway && target.pathname === '/auth/oidc/callback' &&
          method === 'GET' && res.statusCode === 200 &&
          /^text\/html(?:;|$)/.test(res.headers['content-type'] || '')
        const byteLimit = callbackHtml ? 262144 : 65536
        let size = 0
        const chunks = []
        res.on('data', chunk => {
          size += chunk.length
          if (size > byteLimit) {
            req.destroy(new Error('PROVIDER_RESPONSE_BYTE_BUDGET'))
            return
          }
          chunks.push(chunk)
        })
        res.on('error', reject)
        res.on('end', () => {
          const text = Buffer.concat(chunks).toString()
          onReceipt({...metadata, stage: 'response'})
          resolve({status: res.statusCode, headers: res.headers, text, metadata})
        })
      })
      req.once('error', error => {
        const code = /^PROVIDER_[A-Z0-9_]+$/.test(error.message)
          ? error.message
          : 'PROVIDER_TRANSPORT_FAILED'
        onReceipt({...metadata, stage: 'failed', code})
        reject(Object.assign(new Error(code), {requestMetadata: metadata}))
      })
      req.once('timeout', () => req.destroy(new Error('PROVIDER_DEADLINE')))
      req.end(body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body))
    })
  }

  function check(response, status) {
    if (response.status !== status) {
      const code = `PROVIDER_PUBLIC_CONTRACT_${status}_OBSERVED_${response.status}`
      onReceipt({...response.metadata, stage: 'failed', code})
      throw Object.assign(new Error(code), {requestMetadata: response.metadata})
    }
    return response
  }

  async function login(account, {wrongIssuer = false} = {}) {
    const verifier = randomBytes(32).toString('base64url')
    const startResponse = await call(`${gateway}/auth/desktop/start`, {
      method: 'POST',
      origin: gateway,
      body: {
        code_challenge: createHash('sha256').update(verifier).digest('base64url'),
        return_to: '/',
      },
    })
    const start = JSON.parse(check(startResponse, 201).text)
    const authorization = check(await call(start.authorization_url), 303)
    const page = check(await call(authorization.headers.location), 200)
    const transaction = page.text.match(/name="transaction" value="([a-f0-9]+)"/)?.[1]
    if (!transaction) throw new Error('PROVIDER_LOGIN_TRANSACTION_MISSING')
    const loginResponse = check(await call(`${issuer.origin}/login`, {
      method: 'POST',
      body: new URLSearchParams({transaction, email: account.email, password: account.password}).toString(),
    }), 302)
    const callback = new URL(loginResponse.headers.location)
    if (wrongIssuer) callback.searchParams.set('iss', 'https://untrusted.invalid')
    const completed = await call(callback.href)
    if (wrongIssuer) {
      requireWrongIssuerRejection(completed)
      return
    }
    check(completed, 200)
    const pollResponse = check(await call(`${gateway}/auth/desktop/poll`, {
      method: 'POST',
      body: {poll_token: start.poll_token},
    }), 200)
    if (!JSON.parse(pollResponse.text).ready) throw new Error('PROVIDER_HANDOFF_NOT_READY')
    const redeem = check(await call(`${gateway}/auth/desktop/redeem`, {
      method: 'POST',
      origin: gateway,
      body: {redeem_code: start.redeem_code, code_verifier: verifier},
    }), 204)
    const cookies = redeem.headers['set-cookie'] || []
    if (cookies.length !== 1) throw new Error('PROVIDER_SESSION_COOKIE_MISSING')
    const session = {cookie: cookies[0].split(';')[0]}
    sessions.push(session)
    await inspect(session)
    return session
  }

  async function inspect(session) {
    session.view = JSON.parse(check(await call(`${gateway}/auth/session`, session), 200).text)
    return session.view
  }

  async function post(session, path, body) {
    const response = await call(`${gateway}${path}`, {
      method: 'POST', body, cookie: session.cookie, csrf: session.view.csrf_token,
    })
    if (response.status < 200 || response.status >= 300) {
      throw new Error(`PROVIDER_PROVISION_WRITE_FAILED_${response.status}`)
    }
    return response.text ? JSON.parse(response.text) : null
  }

  try {
    await login(issuer.accounts[0], {wrongIssuer: true})
    const owner = await login(issuer.accounts[0])
    if (!owner.view.owner_bootstrap_available) throw new Error('PROVIDER_PRISTINE_BOOTSTRAP_UNAVAILABLE')
    await post(owner, '/v1/access/bootstrap-owner', {
      organization_name: 'Controlled Provider Acceptance', project_name: 'Provider Project',
    })
    await inspect(owner)
    await post(owner, '/v1/auth/allowed-emails', {email: issuer.accounts[1].email, role: 'member'})
    const member = await login(issuer.accounts[1])
    const first = owner.view.authority?.principal_id
    const second = member.view.authority?.principal_id
    if (owner.view.authority_state !== 'ready' || member.view.authority_state !== 'ready' ||
        !first || !second || first === second) {
      throw new Error('PROVIDER_DISTINCT_READY_PRINCIPALS_REQUIRED')
    }
    const team = 'provider-acceptance-team'
    const project = owner.view.projects?.[0]?.id
    if (!owner.view.teams?.length || !project) throw new Error('PROVIDER_BOOTSTRAP_TEAM_PROJECT_MISSING')
    await post(owner, '/v1/access/admin', {
      action: 'access.team.create',
      params: {team_id: team, name: 'Provider Acceptance Team'},
    })
    await post(owner, '/v1/access/admin', {
      action: 'access.team.member.add',
      params: {team_id: team, principal_id: second, role: 'member'},
    })
    await post(owner, '/v1/access/admin', {
      action: 'access.team_project.assign',
      params: {team_id: team, project_id: project, role: 'member'},
    })
    await inspect(member)
    if (!member.view.projects.some(value => value.id === project)) {
      throw new Error('PROVIDER_MEMBER_PROJECT_NOT_VISIBLE')
    }
    return {distinctReadyPrincipals: true, wrongIssuerRejected: true, memberProjectVisible: true, requests}
  } catch (error) {
    provisioningFailure = error
    throw error
  } finally {
    let cleanupFailed = false
    for (const session of sessions) {
      try {
        check(await call(`${gateway}/auth/logout`, {
          method: 'POST', cookie: session.cookie, csrf: session.view.csrf_token,
        }), 204)
      } catch {
        cleanupFailed = true
      } finally {
        session.cookie = ''
        session.view = null
      }
    }
    if (cleanupFailed) {
      onReceipt({stage: 'cleanup', code: 'PROVIDER_PROVISION_LOGOUT_UNCONFIRMED'})
      if (!provisioningFailure) throw new Error('PROVIDER_PROVISION_LOGOUT_UNCONFIRMED')
    }
  }
}
