import {test} from 'node:test'
import assert from 'node:assert/strict'
import {spawn} from 'node:child_process'
import {createServer} from 'node:http'
import {mkdtempSync, readFileSync, rmSync, writeFileSync, readdirSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'

// Removing admission or moving it after Finch transport must break these tests.
async function exercise(mode) {
  const root = mkdtempSync(join(tmpdir(), 'labby-http-guard-'))
  const journal = join(root, 'console-http-counts.json')
  const secret = 'synthetic-private-body-header-query'
  const totals = []
  let requests = 0
  const peer = createServer((request, response) => {
    requests++
    try { totals.push(JSON.parse(readFileSync(journal, 'utf8')).total) } catch {}
    request.resume()
    response.end('ok')
  })
  await new Promise(resolve => peer.listen(0, '127.0.0.1', resolve))
  const fixture = join(root, 'fixture.exs')
  writeFileSync(fixture, `
{:ok, _} = Application.ensure_all_started(:finch)
${mode.startsWith('bootstrap-') ? `
# Evaluate the actual bootstrap's guard prefix without starting its fixed-port endpoint.
bootstrap = "e2e-real/${mode === 'bootstrap-static' ? 'server' : 'provider-server'}.exs"
{:ok, {:__block__, _, expressions}} = bootstrap |> File.read!() |> Code.string_to_quoted()
Code.eval_quoted({:__block__, [], Enum.take(expressions, 2)}, [], file: bootstrap)
` : `
Code.require_file("e2e-real/console-http-guard.exs")
LabbyConsole.E2E.ConsoleHttpGuard.start!(${mode === 'outside-path' ? '"relative-private-path"' : 'System.fetch_env!("LABBY_E2E_CONSOLE_HTTP_RECEIPT")'}, ${mode === 'cap' ? 1 : 2048})
`}
{:ok, _} = Finch.start_link(name: GuardPeer)
url = "http://127.0.0.1:${peer.address().port}"
secret = "${secret}"
request = fn path -> Finch.build(:post, url <> path <> "?private=" <> secret, [{"authorization", secret}], secret) |> Finch.request(GuardPeer) end
case "${mode}" do
  "cap" -> request.("/v1/gateway"); request.("/v1/gateway")
  "concurrent" ->
    1..12 |> Task.async_stream(fn _ -> request.("/v1/gateway") end, max_concurrency: 4) |> Stream.run()
    Finch.build(:get, url <> "/auth/session?private=" <> secret) |> Finch.request(GuardPeer)
    request.("/auth/bearer-session")
    request.("/v1/projects")
    request.("/unknown/" <> secret)
  "write-failure" ->
    File.rm!(System.fetch_env!("LABBY_E2E_CONSOLE_HTTP_RECEIPT"))
    File.mkdir!(System.fetch_env!("LABBY_E2E_CONSOLE_HTTP_RECEIPT"))
    request.("/v1/gateway")
  "malformed" -> :telemetry.execute([:finch, :request, :start], %{}, %{request: secret})
  "bootstrap-static" -> request.("/v1/gateway")
  "bootstrap-provider" -> request.("/v1/gateway")
end
`)
  let stdout = '', stderr = ''
  try {
    const result = await new Promise((resolve, reject) => {
      const child = spawn('mix', ['run', '--no-start', fixture], {
        env: {...process.env, MIX_ENV: 'test', ERL_FLAGS: '+S 4:4', LABBY_E2E_CONSOLE_HTTP_RECEIPT: journal},
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      const timer = setTimeout(() => {child.kill('SIGKILL'); reject(Error('GUARD_CHILD_TIMEOUT'))}, 30000)
      child.stdout.on('data', chunk => {stdout += chunk})
      child.stderr.on('data', chunk => {stderr += chunk})
      child.once('error', reject)
      child.once('exit', (code, signal) => {clearTimeout(timer); resolve({code, signal})})
    })
    let body = ''
    try {body = readFileSync(journal, 'utf8')} catch {}
    assert.equal((stdout + stderr + body).includes(secret), false, 'Guard diagnostics must exclude private request data')
    assert.equal((stdout + stderr).includes('Handler'), false, 'Telemetry must never detach and continue')
    return {result, requests, totals, body, stdout, stderr, files: readdirSync(root)}
  } finally {
    await new Promise(resolve => peer.close(resolve))
    rmSync(root, {recursive: true, force: true})
  }
}

test('Finch guard serializes real concurrent counts and persists only fixed route families', {timeout: 35000}, async () => {
  const run = await exercise('concurrent')
  assert.equal(run.result.code, 0, run.stderr)
  assert.equal(run.requests, 16)
  assert.deepEqual(JSON.parse(run.body), {version: 1, cap: 2048, total: 16,
    gatewayPost: 12, authPreflight: 1, otherServices: 1, other: 2})
  assert.equal(run.totals.every((total, index) => index === 0 || total >= run.totals[index - 1]), true)
  assert.equal(Buffer.byteLength(run.body) <= 4096, true)
  assert.deepEqual(run.files.sort(), ['console-http-counts.json', 'fixture.exs'])
})

test('Finch cap allows one real peer request and halts the owned child before the second transport', {timeout: 35000}, async () => {
  const run = await exercise('cap')
  assert.equal(run.result.code, 86)
  assert.equal(run.requests, 1)
  assert.equal(JSON.parse(run.body).total, 1)
  assert.match(run.stderr, /E2E_CONSOLE_HTTP_BUDGET_EXHAUSTED/)
})

for (const mode of ['malformed', 'write-failure', 'outside-path']) {
  test(`Finch ${mode} halts safely without transport or telemetry detach`, {timeout: 35000}, async () => {
    const run = await exercise(mode)
    assert.equal(run.result.code, 87)
    assert.equal(run.requests, 0)
    assert.match(run.stderr, /E2E_CONSOLE_HTTP_GUARD_FAILED/)
  })
}

for (const mode of ['bootstrap-static', 'bootstrap-provider']) {
  test(`${mode} consumes the guard before application startup`, {timeout: 35000}, async () => {
    const run = await exercise(mode)
    assert.equal(run.result.code, 0, run.stderr)
    assert.equal(run.requests, 1)
    assert.equal(JSON.parse(run.body).gatewayPost, 1)
  })
}
