import {spawnSync} from 'node:child_process'
import {readFileSync} from 'node:fs'
import {resolve} from 'node:path'

if (process.env.LABBY_E2E_BINARY) {
  console.log('Real E2E: using explicitly selected binary; no current-source build claim.')
} else {
  const root = resolve('../..')
  const pin = readFileSync(resolve(root, 'rust-toolchain.toml'), 'utf8').match(/^channel\s*=\s*"([^"]+)"/m)?.[1]
  const version = spawnSync('rustc', ['--version'], {encoding: 'utf8'})
  if (!pin || !version.stdout?.startsWith(`rustc ${pin} `)) {
    throw new Error(`Select the repository Rust ${pin} toolchain before running real E2Es.`)
  }
  const build = spawnSync('cargo', ['build', '--locked', '-p', 'labby', '--bin', 'labby'], {cwd: root, stdio: 'inherit'})
  if (build.status !== 0) process.exit(build.status || 1)
}

const assets = spawnSync('mix', ['assets.deploy'], {stdio: 'inherit'})
if (assets.status !== 0) process.exit(assets.status || 1)
