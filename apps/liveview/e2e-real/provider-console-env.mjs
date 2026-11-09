import {statSync} from 'node:fs'
import {join, resolve} from 'node:path'

export function providerConsoleEnv(parent, {home, temporary}) {
  const mixHome = parent.MIX_HOME || (parent.HOME && join(parent.HOME, '.mix'))
  const archives = parent.MIX_ARCHIVES || (mixHome && join(mixHome, 'archives'))
  try {
    if (!archives || !statSync(archives).isDirectory()) throw new Error('Unavailable archive directory')
  } catch {
    // Keep operator paths and filesystem errors out of the provider receipt.
    throw new Error('PROVIDER_MIX_ARCHIVES_UNAVAILABLE')
  }
  return {
    PATH: parent.PATH,
    HOME: home,
    TMPDIR: temporary,
    LANG: parent.LANG || 'C.UTF-8',
    MIX_ENV: 'test',
    MIX_ARCHIVES: resolve(archives),
    ERL_FLAGS: parent.ERL_FLAGS || '+S 4:4',
  }
}
