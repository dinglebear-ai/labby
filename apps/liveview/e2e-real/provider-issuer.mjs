import {createServer} from 'node:https'
import {generateKeyPairSync, randomBytes, createHash, sign, timingSafeEqual, X509Certificate} from 'node:crypto'
import {mkdtempSync, readFileSync, rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {execFileSync} from 'node:child_process'

const random = () => randomBytes(32).toString('hex')
function equal(first, second) {
  return typeof first === 'string' && typeof second === 'string' &&
    Buffer.byteLength(first) === Buffer.byteLength(second) &&
    timingSafeEqual(Buffer.from(first), Buffer.from(second))
}

export function prepareProviderTls() {
  const root = mkdtempSync(join(tmpdir(), 'labby-controlled-idp-'))
  try {
    execFileSync('openssl', [
      'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
      '-keyout', join(root, 'key.pem'), '-out', join(root, 'cert.pem'),
      '-subj', '/CN=127.0.0.1', '-addext', 'subjectAltName=IP:127.0.0.1',
      '-addext', 'basicConstraints=critical,CA:FALSE',
    ], {stdio: 'ignore', timeout: 15000})
    const certificate = new X509Certificate(readFileSync(join(root, 'cert.pem')))
    const spki = createHash('sha256')
      .update(certificate.publicKey.export({type: 'spki', format: 'der'}))
      .digest('base64')
    return {root, spki}
  } catch (error) {
    rmSync(root, {recursive: true, force: true})
    throw error
  }
}

export async function startProviderIssuer({
  port = 0,
  redirectUri = 'http://127.0.0.1:4329/auth/oidc/callback',
  clientId = 'controlled-provider',
  clientSecret = random(),
  ownerPassword = random(),
  memberPassword = random(),
  tlsDirectory,
} = {}) {
  const root = tlsDirectory || prepareProviderTls().root
  let server
  let timer
  try {
    const ca = readFileSync(join(root, 'cert.pem'))
    const {privateKey, publicKey} = generateKeyPairSync('rsa', {modulusLength: 2048})
    const kid = random()
    const jwk = {...publicKey.export({format: 'jwk'}), kid, use: 'sig', alg: 'RS256'}
    const accounts = [
      {email: 'owner@controlled.invalid', password: ownerPassword, subject: 'controlled-owner'},
      {email: 'member@controlled.invalid', password: memberPassword, subject: 'controlled-member'},
    ]
    const pending = new Map()
    const codes = new Map()
    const counters = {authorized: 0, exchanged: 0, rejected: 0}
    let requests = 0
    let active = 0
    let origin

    function sweep() {
      for (const store of [pending, codes]) {
        for (const [key, value] of store) {
          if (value.expires <= Date.now()) store.delete(key)
        }
      }
    }
    timer = setInterval(sweep, 10000)
    timer.unref()

    server = createServer({key: readFileSync(join(root, 'key.pem')), cert: ca, maxHeaderSize: 8192}, async (req, res) => {
      function finish(status, body, type = 'application/json') {
        res.writeHead(status, {
          'content-type': type,
          'cache-control': 'no-store',
          'x-content-type-options': 'nosniff',
        })
        res.end(typeof body === 'string' ? body : JSON.stringify(body))
      }
      if (++requests > 2000 || active >= 16) {
        finish(429, {error: 'budget_exceeded'})
        return
      }
      active++
      res.once('close', () => active--)
      try {
        sweep()
        if (req.url.length > 4096) {
          finish(414, {error: 'request_too_large'})
          return
        }
        const url = new URL(req.url, origin)
        if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
          finish(200, {
            issuer: origin,
            authorization_endpoint: `${origin}/authorize`,
            token_endpoint: `${origin}/token`,
            jwks_uri: `${origin}/jwks.json`,
            response_types_supported: ['code'],
            subject_types_supported: ['public'],
            id_token_signing_alg_values_supported: ['RS256'],
            token_endpoint_auth_methods_supported: ['client_secret_basic', 'client_secret_post'],
            code_challenge_methods_supported: ['S256'],
          })
          return
        }
        if (req.method === 'GET' && url.pathname === '/jwks.json') {
          finish(200, {keys: [jwk]})
          return
        }
        if (req.method === 'GET' && url.pathname === '/authorize') {
          const params = url.searchParams
          if (params.get('client_id') !== clientId || params.get('redirect_uri') !== redirectUri ||
              params.get('response_type') !== 'code' || params.get('code_challenge_method') !== 'S256' ||
              !/^[-_a-zA-Z0-9]{43}$/.test(params.get('code_challenge') || '') ||
              !params.get('nonce') || !params.get('state') || pending.size >= 32) {
            finish(400, {error: 'invalid_request'})
            return
          }
          const transaction = random()
          pending.set(transaction, {
            challenge: params.get('code_challenge'),
            nonce: params.get('nonce'),
            state: params.get('state'),
            expires: Date.now() + 60000,
          })
          finish(200, `<!doctype html>
<title>Controlled test identity provider</title>
<form method="post" action="/login">
  <input type="hidden" name="transaction" value="${transaction}">
  <label>Email<input name="email" type="email" required></label>
  <label>Password<input name="password" type="password" required></label>
  <button>Sign in</button>
</form>`, 'text/html; charset=utf-8')
          return
        }
        if (req.method !== 'POST' || !['/login', '/token'].includes(url.pathname)) {
          finish(404, {error: 'not_found'})
          return
        }
        let size = 0
        const chunks = []
        for await (const chunk of req) {
          size += chunk.length
          if (size > 8192) {
            finish(413, {error: 'request_too_large'})
            return
          }
          chunks.push(chunk)
        }
        const params = new URLSearchParams(Buffer.concat(chunks).toString())
        if (url.pathname === '/login') {
          const transaction = params.get('transaction')
          const state = pending.get(transaction)
          const account = accounts.find(account =>
            equal(account.email, params.get('email')) && equal(account.password, params.get('password')))
          if (!state || !account || codes.size >= 32) {
            counters.rejected++
            finish(400, {error: 'invalid_login'})
            return
          }
          pending.delete(transaction)
          const code = random()
          codes.set(code, {...state, account, expires: Date.now() + 60000})
          counters.authorized++
          const callback = new URL(redirectUri)
          callback.searchParams.set('code', code)
          callback.searchParams.set('state', state.state)
          res.writeHead(302, {location: callback.href, 'cache-control': 'no-store'})
          res.end()
          return
        }
        let suppliedId = params.get('client_id')
        let suppliedSecret = params.get('client_secret')
        if (req.headers.authorization?.startsWith('Basic ')) {
          const basic = Buffer.from(req.headers.authorization.slice(6), 'base64').toString()
          const separator = basic.indexOf(':')
          suppliedId = decodeURIComponent(basic.slice(0, separator))
          suppliedSecret = decodeURIComponent(basic.slice(separator + 1))
        }
        const code = params.get('code')
        const entry = codes.get(code)
        const verifier = params.get('code_verifier') || ''
        const challenge = createHash('sha256').update(verifier).digest('base64url')
        if (!entry || params.get('grant_type') !== 'authorization_code' ||
            !equal(suppliedId, clientId) || !equal(suppliedSecret, clientSecret) ||
            params.get('redirect_uri') !== redirectUri ||
            !/^[-._~a-zA-Z0-9]{43,128}$/.test(verifier) || !equal(challenge, entry.challenge)) {
          counters.rejected++
          finish(400, {error: 'invalid_grant'})
          return
        }
        codes.delete(code)
        const now = Math.floor(Date.now() / 1000)
        const header = Buffer.from(JSON.stringify({alg: 'RS256', kid, typ: 'JWT'})).toString('base64url')
        const payload = Buffer.from(JSON.stringify({
          iss: origin,
          aud: clientId,
          sub: entry.account.subject,
          email: entry.account.email,
          email_verified: true,
          nonce: entry.nonce,
          iat: now,
          exp: now + 120,
        })).toString('base64url')
        const unsigned = `${header}.${payload}`
        const signature = sign('RSA-SHA256', Buffer.from(unsigned), privateKey).toString('base64url')
        counters.exchanged++
        finish(200, {
          access_token: random(),
          id_token: `${unsigned}.${signature}`,
          token_type: 'Bearer',
          expires_in: 120,
          scope: 'openid email profile',
        })
      } catch {
        if (!res.headersSent) finish(400, {error: 'invalid_request'})
        else res.destroy()
      }
    })
    server.requestTimeout = 5000
    server.headersTimeout = 5000
    server.keepAliveTimeout = 1000
    server.maxConnections = 16
    await new Promise((resolve, reject) => {
      server.once('error', reject)
      server.listen(port, '127.0.0.1', resolve)
    })
    origin = `https://127.0.0.1:${server.address().port}`
    return {
      origin, ca, caPath: join(root, 'cert.pem'), clientId, clientSecret, redirectUri, accounts, counters,
      spki: createHash('sha256')
        .update(new X509Certificate(ca).publicKey.export({type: 'spki', format: 'der'}))
        .digest('base64'),
      async close() {
        clearInterval(timer)
        pending.clear()
        codes.clear()
        server.closeAllConnections()
        await new Promise(resolve => server.close(resolve))
        if (!tlsDirectory) rmSync(root, {recursive: true, force: true})
      },
    }
  } catch (error) {
    clearInterval(timer)
    server?.close()
    if (!tlsDirectory) rmSync(root, {recursive: true, force: true})
    throw error
  }
}
