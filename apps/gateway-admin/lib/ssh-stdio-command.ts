import { formatStdioCommandLine, parseStdioCommandLine } from './stdio-command'

const SSH_OPTIONS = ['-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10']

export function sshStdioCommand(host: string, remoteCommand: string): string {
  if (!host || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(host) || !remoteCommand.trim()) return ''
  return formatStdioCommandLine('ssh', [...SSH_OPTIONS, host, remoteCommand])
}

export function selectedSshStdioHost(commandLine: string): { host: string; remoteCommand: string } | null {
  try {
    const parsed = parseStdioCommandLine(commandLine)
    if (parsed.command !== 'ssh') return null
    if (parsed.args.length !== SSH_OPTIONS.length + 2) return null
    if (!SSH_OPTIONS.every((option, index) => parsed.args[index] === option)) return null
    const host = parsed.args[SSH_OPTIONS.length]!
    const remoteCommand = parsed.args[SSH_OPTIONS.length + 1]!
    return sshStdioCommand(host, remoteCommand) ? { host, remoteCommand } : null
  } catch {
    return null
  }
}
