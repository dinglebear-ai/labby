#!/usr/bin/env node
import { writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { spawn } from 'node:child_process'

const marker = process.env.LABBY_E2E_BROWSER_FIXTURE_MARKER
if (!marker) throw new Error('owned browser fixture marker required')
if (process.env.LABBY_E2E_BROWSER_LEADER_EXIT === '1' && !process.env.LABBY_E2E_BROWSER_FIXTURE_CHILD) {
  const child = spawn(process.execPath, [process.argv[1]], {
    env: { ...process.env, LABBY_E2E_BROWSER_FIXTURE_CHILD: '1', LABBY_E2E_BROWSER_FIXTURE_GROUP: String(process.ppid) },
    stdio: ['ignore', 'ignore', 'ignore', 'ipc'],
  })
  child.once('message', () => process.exit(0))
} else {
  process.on('SIGTERM', () => {})
  const server = createServer()
  server.listen(0, '127.0.0.1', () => {
    writeFileSync(marker, JSON.stringify({ pid: process.pid, group: Number(process.env.LABBY_E2E_BROWSER_FIXTURE_GROUP ?? process.ppid), port: server.address().port }), { mode: 0o600 })
    process.send?.('ready')
  })
}
