import test from 'node:test'
import assert from 'node:assert/strict'
import { selectedSshStdioHost, sshStdioCommand } from './ssh-stdio-command'
import { parseStdioCommandLine } from './stdio-command'

test('selected SSH device keeps the remote command as one argument', () => {
  assert.deepEqual(parseStdioCommandLine(sshStdioCommand('tootie', '/usr/local/bin/mcp serve')), {
    command: 'ssh',
    args: ['-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', 'tootie', '/usr/local/bin/mcp serve'],
  })
  assert.equal(sshStdioCommand('-ProxyCommand=bad', 'mcp serve'), '')
  assert.equal(sshStdioCommand('tootie', '  '), '')
  assert.deepEqual(selectedSshStdioHost(sshStdioCommand('tootie', '/usr/local/bin/mcp serve')), {
    host: 'tootie', remoteCommand: '/usr/local/bin/mcp serve',
  })
  assert.equal(selectedSshStdioHost('npx -y mcp-server'), null)
})
