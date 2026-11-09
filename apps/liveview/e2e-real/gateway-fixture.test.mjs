import {test} from 'node:test'
import assert from 'node:assert/strict'
import {spawn} from 'node:child_process'
import {mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join, dirname, basename} from 'node:path'

for (const mode of ['missing-frame', 'test-abort', 'browser-close-failure']) {
  test(`Gateway fixture reaches terminal resource cleanup after ${mode}`, {timeout: 60000}, async () => {
    const directory = mkdtempSync(join(tmpdir(), 'labby-gateway-cleanup-'))
    const ready = join(directory, 'ready.json')
    const done = join(directory, 'done.json')
    const script = join(directory, 'failure.mjs')
    const module = new URL('./gateway-fixture.mjs', import.meta.url).href
    writeFileSync(script, `
import {test} from 'node:test'
import {writeFileSync} from 'node:fs'
import {withGatewayFixture, gatewayFrameGate} from ${JSON.stringify(module)}
const controller = new AbortController()
test('owned failure', {signal: controller.signal}, async context => {
 await withGatewayFixture({signal: context.signal,
  onResources: value => writeFileSync(${JSON.stringify(ready)}, JSON.stringify(value)),
  onCleanup: value => writeFileSync(${JSON.stringify(done)}, JSON.stringify(value)),
  ...(${JSON.stringify(mode)} === 'browser-close-failure' ? {closeBrowser: async () => {throw Error('FIXTURE_CLOSE_FAILED')}} : {}),
 }, async () => {
  if (${JSON.stringify(mode)} === 'browser-close-failure') return
  const frame = gatewayFrameGate(context.signal, 100)
  if (${JSON.stringify(mode)} === 'test-abort') controller.abort()
  await frame.wait
 })
})
`)
    const env = {...process.env}
    delete env.NODE_TEST_CONTEXT
    const child = spawn(process.execPath, ['--test', script], {cwd: process.cwd(), env, stdio: 'ignore'})
    let timedOut = false
    const timer = setTimeout(() => {timedOut = true; child.kill('SIGKILL')}, 45000)
    try {
      const code = await new Promise(resolve => child.once('exit', resolve))
      assert.equal(timedOut, false, 'failure handling must terminate without the external watchdog')
      assert.equal(code, 1)
      assert.equal(existsSync(ready), true, 'real browser and Mix app must have started')
      assert.deepEqual(JSON.parse(readFileSync(done, 'utf8')), {browserDisconnected: true, childExited: true, directoryRemoved: true})
      const resources = JSON.parse(readFileSync(ready, 'utf8'))
      assert.equal(typeof resources.mixPid, 'number')
      assert.equal(typeof resources.browserPid, 'number')
      for (const pid of [resources.mixPid, resources.browserPid]) {
        assert.throws(() => process.kill(pid, 0), {code: 'ESRCH'})
      }
    } finally {
      clearTimeout(timer)
      if (existsSync(ready)) {
        const resources = JSON.parse(readFileSync(ready, 'utf8'))
        for (const pid of [resources.mixPid, resources.browserPid].filter(Number.isInteger)) {
          try {process.kill(pid, 'SIGKILL')} catch (error) {if (error.code !== 'ESRCH') throw error}
        }
        if (dirname(resources.directory) === tmpdir() && basename(resources.directory).startsWith('labby-gateway-settled-')) {
          rmSync(resources.directory, {recursive: true, force: true})
        }
      }
      rmSync(directory, {recursive: true, force: true})
    }
  })
}
