import {spawn} from 'node:child_process'
import {mkdtempSync, writeFileSync, rmSync, existsSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {chromium} from '@playwright/test'

export function gatewayFrameGate(signal, timeout = 5000) {
  let accept
  const wait = new Promise((resolve, reject) => {
    const finish = (operation, value) => {
      clearTimeout(timer)
      signal?.removeEventListener('abort', abort)
      operation(value)
    }
    const abort = () => finish(reject, Error('GATEWAY_FRAME_ABORTED'))
    const timer = setTimeout(() => finish(reject, Error('GATEWAY_FRAME_TIMEOUT')), timeout)
    accept = () => finish(resolve)
    signal?.addEventListener('abort', abort, {once: true})
    if (signal?.aborted) abort()
  })
  return {wait, accept}
}

export async function withGatewayFixture({signal, onResources = () => {}, onCleanup = () => {}, closeBrowser = browser => browser.close()}, run) {
  const directory = mkdtempSync(join(tmpdir(), 'labby-gateway-settled-'))
  const script = join(directory, 'server.exs')
  writeFileSync(script, `
ExUnit.start(autorun: false)
Code.require_file("test/labby_console_web/gateway_mutations_live_test.exs")
owner = self()
{:ok, state} = Agent.start_link(fn -> %{owner: owner, session_status: 200, response: {200, "{}"}, rows: %{}} end)
{:ok, fixture} = Bandit.start_link(plug: {LabbyConsole.GatewayMutationFixture, state: state}, port: 0, ip: {127,0,0,1})
{:ok, {_, port}} = ThousandIsland.listener_info(fixture)
Application.put_env(:labby_console, :gateway_url, "http://127.0.0.1:#{port}")
Application.put_env(:labby_console, LabbyConsoleWeb.Endpoint, Keyword.merge(Application.get_env(:labby_console, LabbyConsoleWeb.Endpoint), server: true, http: [ip: {127,0,0,1}, port: 0], check_origin: ["//127.0.0.1"]))
{:ok, _} = Application.ensure_all_started(:labby_console)
{:ok, {_, port}} = LabbyConsoleWeb.Endpoint.server_info(:http)
IO.puts("GATEWAY_HELPER_READY #{port}")
Process.sleep(:infinity)
`)
  const child = spawn('mix', ['run', '--no-start', script], {
    env: {...process.env, MIX_ENV: 'test', ERL_FLAGS: '+S 4:4'},
    stdio: ['ignore', 'pipe', 'ignore'],
  })
  const exited = new Promise(resolve => {
    child.once('exit', resolve)
    child.once('error', resolve)
  })
  let browser
  let browserServer
  const abort = () => {
    child.kill('SIGTERM')
    if (browserServer) void browserServer.kill().catch(() => {})
  }
  signal?.addEventListener('abort', abort, {once: true})
  try {
    signal?.throwIfAborted()
    onResources({mixPid: child.pid, directory})
    const port = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(Error('GATEWAY_HELPER_START_TIMEOUT')), 30000)
      let output = ''
      child.stdout.on('data', chunk => {
        output = (output + chunk).slice(-16384)
        const match = output.match(/GATEWAY_HELPER_READY (\d+)/)
        if (match) {clearTimeout(timer); resolve(Number(match[1]))}
      })
      child.once('error', () => {clearTimeout(timer); reject(Error('GATEWAY_HELPER_START_FAILED'))})
      child.once('exit', () => {clearTimeout(timer); reject(Error('GATEWAY_HELPER_EARLY_EXIT'))})
    })
    browserServer = await chromium.launchServer({executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH, timeout: 10000})
    onResources({mixPid: child.pid, browserPid: browserServer.process().pid, directory})
    signal?.throwIfAborted()
    browser = await chromium.connect(browserServer.wsEndpoint(), {timeout: 10000})
    signal?.throwIfAborted()
    return await run({browser, port})
  } finally {
    signal?.removeEventListener('abort', abort)
    try {
      if (browser) {
        let timer
        try {
          await Promise.race([closeBrowser(browser), new Promise((_, reject) => {
            timer = setTimeout(() => reject(Error('GATEWAY_BROWSER_CLOSE_TIMEOUT')), 5000)
          })])
        } finally {
          clearTimeout(timer)
        }
      }
    } finally {
      try {
        if (browserServer) await browserServer.kill()
      } finally {
        child.kill('SIGTERM')
        let timer
        try {
          await Promise.race([exited, new Promise(resolve => {timer = setTimeout(resolve, 5000)})])
        } finally {
          clearTimeout(timer)
        }
        if (child.pid && child.exitCode === null && child.signalCode === null) {
          child.kill('SIGKILL')
          try {
            await Promise.race([exited, new Promise((_, reject) => {
              timer = setTimeout(() => reject(Error('GATEWAY_HELPER_STOP_TIMEOUT')), 5000)
            })])
          } finally {
            clearTimeout(timer)
          }
        }
        rmSync(directory, {recursive: true, force: true})
        onCleanup({browserDisconnected: !browser?.isConnected(), childExited: !child.pid || child.exitCode !== null || child.signalCode !== null, directoryRemoved: !existsSync(directory)})
      }
    }
  }
}
